---
name: 'Channel HISTORY tab: the user''s messages, summarized, jump to each'
overview: A HISTORY tab after TASKS in a channel's right panel lists every message the user posted in the channel, newest first, with its time. Long ones get a one-line Haiku summary that is cached on disk, and clicking or pressing Enter on one scrolls the chat to that message and flashes it.
todos:
- id: ui-types
  content: 'aura-ui: POST_SUMMARY_MIN_CHARS, UiCommand::SetPostSummaries'
  status: pending
- id: tab
  content: 'Terminal: ChannelHistory tab after TASKS, list state (selection, wheel scroll, summaries), keys and clicks'
  status: pending
- id: render
  content: 'Renderer: HISTORY list rows (text/summary or word-cut fallback, time); rename drawer row'
  status: pending
- id: jump
  content: 'Chat jump: ChatLineCache::row_of, pending jump, scroll and flash in layout_rows'
  status: pending
- id: app
  content: 'aura-app: summaries service (cache file, in-flight set, Haiku via titles path, SetPostSummaries)'
  status: pending
- id: tests
  content: 'Tests: placement, user-only, threshold, fallback, cache, summarize stubs, jump, wheel'
  status: pending
- id: docs
  content: STYLEGUIDE.md, help/docs
  status: pending
- id: ship
  content: fmt/clippy/tests; branch, push, PR, merge latest main, retest, merge
  status: pending
---

# Channel HISTORY tab: the user's messages, summarized, jump to each

A HISTORY tab after TASKS in a channel's right panel lists every message the user posted in the channel, newest first, with its time. Long ones get a one-line Haiku summary that is cached on disk, and clicking or pressing Enter on one scrolls the chat to that message and flashes it.

## Todos

- [ ] aura-ui: POST_SUMMARY_MIN_CHARS, UiCommand::SetPostSummaries
- [ ] Terminal: ChannelHistory tab after TASKS, list state (selection, wheel scroll, summaries), keys and clicks
- [ ] Renderer: HISTORY list rows (text/summary or word-cut fallback, time); rename drawer row
- [ ] Chat jump: ChatLineCache::row_of, pending jump, scroll and flash in layout_rows
- [ ] aura-app: summaries service (cache file, in-flight set, Haiku via titles path, SetPostSummaries)
- [ ] Tests: placement, user-only, threshold, fallback, cache, summarize stubs, jump, wheel
- [ ] STYLEGUIDE.md, help/docs
- [ ] fmt/clippy/tests; branch, push, PR, merge latest main, retest, merge

<!-- end of generated header: edit below -->

## Where it goes and what it lists

A channel's side tabs are MEMBERS, INFO, QUEUE, TASKS. HISTORY goes after TASKS, which is what the user asked for. Its short label is `HIS`, because HOST already uses `HST`.

No `*history*` branch exists on origin, so this starts from main.

The list is the open channel's posts whose author is the user. They are already in the UI, in `ChannelState.posts`. Each item takes two rows:
- the message on one line, with its line breaks folded into spaces;
- its time in `muted`: `20:31` for today, `11-27 20:31` otherwise.

The list is newest first, since the latest messages are the likely ones to go back to. The list's header says `Your messages · N`.

**Naming:** the to-do drawer's `Task history` row is renamed `Finished to-dos`. The two features then share no wording.

## Summaries

- **Threshold:** `POST_SUMMARY_MIN_CHARS = 80`, in `aura-ui` (`events/views/channels.rs`), so the app and the UI use the same number. A message of at most 80 characters shows as it is, cut to the row's width at a word with `…` when it doesn't fit.
- **Generation** (`aura-app`, new `event_loop/channels/summaries.rs`):
  - Whenever the posts go to the UI (`send_posts_of` when the coordinator's chat loads, `send_post` for a new post), each user post over the threshold gets its summary.
  - A cached summary is sent at once. Otherwise one model call is spawned per text, tracked in a set of calls in flight so a text is never asked twice.
  - The call reuses the title path (`titles::ask` with a new `TitleKind::Summary`), so it goes through the coordinator's kernel like titles and has the 15-second `TITLE_TIMEOUT`.
  - The input is capped at 2,000 characters, thinking is off and the reply is cut to 60 characters.
  - The model is the first Claude Haiku among the user's routes, else the cheap title model.
  - Without a kernel or a model, there is no call.
- **Cache:** `<data dir>/channel-post-summaries.json`, keyed by the blake3 hash of the text.
  - It is read and written on a blocking thread and replaced atomically.
  - It is capped at 2,000 entries, dropping the oldest.
  - Because it is keyed by text, it covers every channel and survives restarts.
- **To the UI:** a new `UiCommand::SetPostSummaries { channel_id, summaries: Vec<(u64, String)> }` maps post seq to summary. The UI keeps summaries per channel.
- **Fallback:** until a summary comes, or if the call fails, the message is cut at a word with `…`.

## Jump to a message

- **Starting a jump:** a click on an item, or Enter on the highlighted one, asks the chat to jump to that post.
  - A member's own chat on screen goes back to the channel's chat first.
  - Focus stays in the panel, so you can keep moving through the list.
- **Finding the message:** each post is its own block in `ChatLineCache`, under the key `POST_ID | seq`.
  - The cache gets `row_of(key)`, giving the block's first row and its length.
  - In `layout_rows`, a jump that is waiting sets the scroll so the message starts one row below the top, clamped like any scroll. This works at any depth of the scrollback.
  - It then flashes the message's rows with the `selection` background for 1.2 seconds, and `chat_animating` stays true during the flash so it redraws.
  - A jump whose post isn't in the chat yet (still loading) waits up to 3 seconds, then is dropped.
- **Keys:** ↑↓, PgUp/PgDn and Home/End move the highlight, and Enter jumps.
- **Mouse wheel:** it scrolls the list only and never moves the highlight. FEN6's wheel change for the other lists isn't on main yet, so this tab follows that rule on its own.

## Code

- `aura-ui`: the threshold, `UiCommand::SetPostSummaries`.
- `aura-ui-terminal`:
  - `SidePanelTab::ChannelHistory` and every match on it;
  - `app/apps/channels/history.rs`: the list, its selection and scroll, summaries, clicks and keys;
  - `renderer/channel_history.rs`;
  - the chat jump in `app/chat_view.rs` and `renderer/chat_panel/rows.rs`;
  - `ChatLineCache::row_of`;
  - the drawer row renamed;
  - `STYLEGUIDE.md`.
- `aura-app`: `channels/summaries.rs`, `titles` `TitleKind::Summary`, the calls from `send_posts_of` and `send_post`, and the prompt in `aura-context-prompts`.

## Tests (model calls stubbed)

- **Placement:** HISTORY is right after TASKS; click and Shift+arrows reach it.
- **User-only:** only the user's posts are listed, newest first, with times.
- **Threshold and fallback:** a short message shows whole; a long one is cut at a word with `…` until its summary comes, then shows the summary.
- **Summary cache:** it round-trips through the file, keeps its cap, and a cached text is not asked again.
- **Summarize:** with a stub provider, a good reply, a failure and a timeout fall back.
- **Jump:** a click and Enter scroll the chat to a message far back and flash it; the flash ends; the wheel scrolls the list without moving the highlight.

## Unsure

- **Cloud coordinators:** a coordinator in the cloud has no kernel here. Its long messages then keep the cut fallback.
- **What counts as the user's messages:** only posts in the channel's chat. Messages typed in a member's own chat are not channel posts, so they aren't listed.
