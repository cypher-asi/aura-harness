---
name: Channel HISTORY tab
overview: Add a HISTORY tab after TASKS in a channel's right panel. It lists every message the user sent in the channel. Long messages get a cached one-line Haiku summary and fall back to a cut preview. Clicking a row or pressing Enter jumps the chat to the message and flashes it.
todos:
- id: git-setup
  content: Back up local commit, stash WIP, reset branch to origin/main
  status: pending
- id: protocol
  content: Add SummarizeMessages event, SetMessageSummary command and views in aura-ui
  status: pending
- id: prompt
  content: Add message_summary auxiliary prompt in aura-context-prompts
  status: pending
- id: session
  content: 'aura-app summaries module: cache file, Haiku call with timeout, routing, tests'
  status: pending
- id: tab
  content: Add SidePanelTab::ChannelHistory after Tasks and wire all tab matches
  status: pending
- id: entries
  content: Build history entries, list state, keys, wheel, click, hover
  status: pending
- id: summaries-ui
  content: Threshold, fallback, lazy summary requests and SetMessageSummary handling
  status: pending
- id: render
  content: Render HISTORY rows with times
  status: pending
- id: jump
  content: Pending chat jump resolved in layout_rows, plus flash highlight
  status: pending
- id: tests-ui
  content: aura-ui-terminal tests for placement, listing, threshold, fallback, cache, jump
  status: pending
- id: docs
  content: STYLEGUIDE.md, module docs and CHANGELOG
  status: pending
- id: verify
  content: fmt, clippy, tests for changed crates and aura-desktop, file-size check
  status: pending
- id: ship
  content: Push, open PR, merge origin/main, retest, merge with merge commit, report
  status: pending
---

# Channel HISTORY tab

Add a HISTORY tab after TASKS in a channel's right panel. It lists every message the user sent in the channel. Long messages get a cached one-line Haiku summary and fall back to a cut preview. Clicking a row or pressing Enter jumps the chat to the message and flashes it.

## Todos

- [ ] Back up local commit, stash WIP, reset branch to origin/main
- [ ] Add SummarizeMessages event, SetMessageSummary command and views in aura-ui
- [ ] Add message_summary auxiliary prompt in aura-context-prompts
- [ ] aura-app summaries module: cache file, Haiku call with timeout, routing, tests
- [ ] Add SidePanelTab::ChannelHistory after Tasks and wire all tab matches
- [ ] Build history entries, list state, keys, wheel, click, hover
- [ ] Threshold, fallback, lazy summary requests and SetMessageSummary handling
- [ ] Render HISTORY rows with times
- [ ] Pending chat jump resolved in layout_rows, plus flash highlight
- [ ] aura-ui-terminal tests for placement, listing, threshold, fallback, cache, jump
- [ ] STYLEGUIDE.md, module docs and CHANGELOG
- [ ] fmt, clippy, tests for changed crates and aura-desktop, file-size check
- [ ] Push, open PR, merge origin/main, retest, merge with merge commit, report

<!-- end of generated header: edit below -->

## How it works today

While a channel is open, the right column shows `SidePanelTab::IN_CHANNEL` = MEMBERS, INFO, QUEUE, TASKS (`crates/ui/aura-ui-terminal/src/app/processes/side_tab.rs`). TASKS is already a tab there (df121c5e moved it from a panel into a tab after QUEUE), so HISTORY goes straight after it as the last tab. The channel's chat is the coordinator's chat (`App::messages`) with the channel's posts merged in (`renderer/chat_lines/timeline.rs`). So the user's messages come from two places:

- prompts typed to the coordinator, which are `MessageRole::User` messages;
- `ChannelPostAuthor::User` posts, which are what a message mentioning members becomes.

The chat lays out every block in `ChatLineCache`, at most `MAX_MESSAGES` = 2000. `renderer/chat_panel/rows.rs` (`layout_rows`, `BlockHits::collect`) knows each block's row, so a jump can be resolved exactly. That works however far back the message is, as long as the chat holds it.

Title generation is the cheap model path to reuse. `aura-app/src/event_loop/titles/mod.rs` builds an `aura_agent::titles::request` (Auxiliary kind, thinking off), calls it through `KernelModelGateway` under `tokio::time::timeout(TITLE_TIMEOUT)`, and cleans the answer with `sanitize`. `spawn_subject` sends its result to the UI as a `UiCommand`. I copy that shape.

## Workspace and branch first

The clone has the user's local commit 49d582b9 ("List TASKS by day"), which is not on origin, plus 61 uncommitted files that belong to other work. Neither may end up in this PR, and neither may be lost:

- Keep the local commit on a local-only branch, `backup/local-49d582b9`.
- Put the uncommitted files in `git stash push -m "local WIP before RUS200 history tab"`.
- Point `aura/channel-history-tab-rus200` at `origin/main` with `git checkout -B`.

Nothing of that is pushed. The report will mention it.

## Protocol (aura-ui)

- In `events/views/channels.rs`, add `MessageSummaryRequest { key, text }` and `MessageSummaryView { key, summary: Option<String> }`. A `None` summary means the call failed or timed out.
- Add `UiEvent::SummarizeMessages { agent_id, requests }` to `events/event.rs`, where `agent_id` is the channel's coordinator, whose kernel makes the call.
- Add `UiCommand::SetMessageSummary(MessageSummaryView)` to `events/command.rs`. That file has 496 lines, so the doc comment stays at one line to keep it under 500.

## Summaries in the session (aura-app)

The new module is `event_loop/channels/summaries/` (`mod.rs`, `cache.rs`, `tests.rs`).

- **Routing:** `ui_events/mod.rs` sends the event to `Domain::Places`, and `places.rs` calls `summaries::handle(state, agent_id, requests)`.
- **Cache:** a JSON file at `<data_dir>/message_summaries.json`, where `data_dir` is the registry's data directory, the same place `approvals.json` lives. Entries are keyed by the blake3 hex of the message text and hold the summary and when it was made. There are at most 5,000 entries, and the oldest go first. The file is loaded lazily and saved through `spawn_blocking`, written to a temporary file and then renamed into place. An unreadable file is logged and treated as empty. The cache and an in-flight set live on `AgentHub::channel_chat` (`ChannelChat`), behind a mutex.
- **Request flow:** cached texts are answered at once. Texts already in flight are skipped. For the rest, one spawned task works through them one call at a time, each under `SUMMARY_TIMEOUT` (10 s). Each result is stored, then sent as `SetMessageSummary`; a failure sends `None` and is not cached, so a later run can try again.
- **Model:** the first gate candidate whose id contains `haiku`, else `titles::title_model(classifier)`. Requests go through `KernelModelGateway` on `state.kernel_for(coordinator)`, with `agent_loop.config()`. With no kernel (a cloud coordinator), every request gets `None` back and the UI shows its fallback.
- **Cost:** the model sees at most `MAX_SIDE_CHARS` (2,000) characters of input and may answer with `max_tokens(48)`. The answer is cleaned with `titles::sanitize(…, MAX_SUMMARY_CHARS = 72)`. The core does not take `&dyn ModelProvider`, so tests stub it.
- **Prompt:** a new `aura-context-prompts/src/auxiliary/message_summary.rs` holds `MESSAGE_SUMMARY_SYSTEM_PROMPT` and `message_summary_user_prompt`. The call reuses `ModelCallPurpose::Title` rather than adding a new purpose variant.

## The HISTORY tab (aura-ui-terminal)

**Tab:** `SidePanelTab::ChannelHistory`, labelled `HISTORY` (short `HIS`), appended to `IN_CHANNEL` and counted in `is_channel_tab`. The compiler will point at every exhaustive match that needs the new arm: `processes/mod.rs` (Home/End, click, hover), `scroll.rs`, `keys/panels.rs`, `renderer/sandbox.rs`, and `HoverTarget::ChannelHistoryRow` in `hit_targets.rs`.

**Entries** (`app/apps/channels/history/mod.rs`):

- Each entry is built from the open channel's chat: `MessageRole::User` messages with no shell run, no inbox fold and some text, plus `ChannelPostAuthor::User` posts.
- They are merged in time order, and a message with the same text as a post within 5 s is listed once.
- Each carries a `HistoryKey`, either `Message { at_ms, digest }` or `Post { seq }`, its one-line text and its time.
- The list is rebuilt only when the chat's fingerprint changes (message count, last id, post count). A snapshot is kept for each channel, so it still lists while a member's chat is on screen.
- **Newest is at the bottom**, as in the chat. The tab opens on the newest entry and stays on the newest while new messages arrive, unless the user moved off it.

**Rows** (`renderer/channel_history.rs`): one `list_row` per entry, with the text on the left and a muted time on the right. The time is `HH:MM` for today and `MM-DD HH:MM` otherwise, as QUEUE shows it. An empty list shows "No messages from you yet."

**Threshold** (`history/summaries.rs`): `SUMMARY_THRESHOLD_CHARS = 80` characters, counted after the text is folded to one line.

- Shorter messages show as they are, cut to the row's width.
- Longer ones show their cached summary. While the summary is pending, or after it failed, they show the text cut at a word to 80 characters with `…`.
- Summaries are asked for only for rows as they are drawn, so cost follows what the user looks at. Each key is asked for once per run (the `requested` set), and a failed key is not asked again that run. Results go in `summaries: HashMap<key, String>`.
- The UI key is a `DefaultHasher` hex of the text. It only has to be stable within one run, because the session's file is the cache that lasts.

**Keys, wheel and click:**

- Up and Down, PgUp and PgDn, Home and End move the selection, and Enter jumps.
- A click selects the row and jumps.
- The wheel scrolls the list without moving the selection. It sets `scroll_detached`, as PROJECTS does, until the selection moves again.
- I asked FEN6 whether their change brings a shared helper. If it lands first, I'll use it.

## The jump and the flash

`history/jump.rs`:

- Activating an entry sets `chat_jump = Some((key, Instant))`.
- If a member's chat is on screen, it first calls `close_member_chat()`, and the jump waits for the coordinator's history to load.
- In `layout_rows`, after `BlockHits::collect`, the key is resolved to a message id among the shown messages (`post_message` ids for posts). `ChatLineCache` gets a `block_ids()` accessor so the block's first row can be found.
- `App::jump_chat_to_row(row, total, height)` sets `scroll_offset` so the message sits one row below the top of the chat, clamped at the bottom, and then sets `chat_flash = (id, now)`.
- A jump that does not resolve within 10 s, for example a message trimmed out of the chat, is dropped with a short notification.
- A new `chat_panel/flash.rs` paints the message's visible rows with the `selection` background for `CHAT_FLASH = 1.5 s`. `chat_animating()` stays true while a flash or jump is pending, so frames keep coming.
- To keep the tests clock-free, the "is it still flashing" check takes `now` as an argument.

## Docs

- In `STYLEGUIDE.md`, add HISTORY to the Lists line in Panel Bodies and to the channel bar description. Add a short HISTORY section covering the row format, the threshold and fallback, newest at the bottom, the wheel, and the jump flash.
- Update the module doc of `apps/channels/mod.rs` and `side_tab.rs`.
- Add a CHANGELOG Unreleased entry.

## Tests (models stubbed)

- **aura-ui-terminal:**
  - HISTORY is listed right after TASKS, and only in a channel.
  - Only the user's messages and posts are listed, not the agents' or system ones.
  - The threshold works at 80 and 81 characters.
  - The fallback shows while a summary is pending, and also after `SetMessageSummary` with `None`.
  - A received summary is shown.
  - Each key is requested once.
  - The wheel scrolls without moving the selection.
  - Enter and click set the jump. Drawing a long chat (a test backend, the message far up the scrollback) puts that message's first row on screen and flashes it.
  - The jump from a member's chat goes back to the channel's chat.
- **aura-app:**
  - A cache hit makes no model call (`MockProvider::call_count`).
  - A miss calls once, stores the result, and the stored file round-trips (with `tempfile`).
  - A failure gives `None` and is not cached.
  - A stalled provider times out, tested with paused time.
  - Eviction keeps the newest entries.
  - The Haiku model is picked from the candidates.
- **aura-context-prompts:** the user prompt contains the text.

## Ship

- Run `cargo fmt --all`, then `cargo clippy` with `-D warnings` and `cargo test` for `-p aura-ui -p aura-ui-terminal -p aura-desktop -p aura-app -p aura-context-prompts`, then `bash scripts/check_file_size.sh`.
- Commit, push without force, and `gh pr create`.
- Merge the latest `origin/main` into the branch and rerun the same tests. If tests fail, or a conflict could change behaviour (FEN6's wheel change, RUS197's to-dos panel, RUS199's banner), stop and report to COO2.
- Merge with `gh pr merge --merge`, which makes a normal merge commit.

## Things I'm not sure about

- The message cap: only what the chat holds (the last 2,000 messages) can be listed or jumped to.
- A cloud coordinator has no local kernel, so it gets fallbacks only.
- Whether any typed user message carries a marker I'm not filtering out yet. The tests will pin down what is listed.
