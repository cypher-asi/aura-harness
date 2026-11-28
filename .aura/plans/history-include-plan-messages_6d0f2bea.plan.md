---
name: 'HISTORY: include /plan messages'
overview: Record the text the user sends with `/plan` in a channel as a hidden "plan post" that names its planner. HISTORY lists it with the other messages, summarizes it the same way, and a jump opens the planner's chat at that message. The channel's chat and the coordinator's digest stay as they are.
todos:
- id: store
  content: Add optional planner to ChannelPost/NewPost and ChannelPostView
  status: pending
- id: session
  content: start_planner stores a plan post, sends it, asks its summary; digest skips it
  status: pending
- id: ui-posts
  content: File plan posts apart from the chat's posts in the UI
  status: pending
- id: ui-history
  content: List plan items in HISTORY with summaries, ordered and de-duplicated
  status: pending
- id: ui-jump
  content: Jump opens the planner's chat at the request, notice when gone
  status: pending
- id: tests
  content: Tests for store, session, listing, de-dup, summaries and jump
  status: pending
- id: docs
  content: STYLEGUIDE, module docs and CHANGELOG
  status: pending
- id: ship
  content: Verify, push, PR, merge origin/main, retest, merge commit, report
  status: pending
---

# HISTORY: include /plan messages

Record the text the user sends with `/plan` in a channel as a hidden "plan post" that names its planner. HISTORY lists it with the other messages, summarizes it the same way, and a jump opens the planner's chat at that message. The channel's chat and the coordinator's digest stay as they are.

## Todos

- [ ] Add optional planner to ChannelPost/NewPost and ChannelPostView
- [ ] start_planner stores a plan post, sends it, asks its summary; digest skips it
- [ ] File plan posts apart from the chat's posts in the UI
- [ ] List plan items in HISTORY with summaries, ordered and de-duplicated
- [ ] Jump opens the planner's chat at the request, notice when gone
- [ ] Tests for store, session, listing, de-dup, summaries and jump
- [ ] STYLEGUIDE, module docs and CHANGELOG
- [ ] Verify, push, PR, merge origin/main, retest, merge commit, report

<!-- end of generated header: edit below -->

## Why it is missing today

`/plan <text>` in a channel's chat sends `UiEvent::PlanInChannel`. `start_planner` (`aura-app/src/event_loop/channels/plan.rs`) adds a new member, puts its chat on screen and delivers the text there. Nothing is stored in the channel, and the coordinator's chat deliberately shows nothing of it (`request_channel_plan` says so). HISTORY reads the channel's posts and the coordinator's chat, so it never sees the text. Reading every planner's own chat would not survive a restart, because the UI only holds chats it has shown.

## The approach: a hidden plan post

The channel store already keeps the user's posts durably as JSON with serde defaults. One more optional field marks a post as a `/plan` request and names its planner, so old rows still read.

- `aura-store-db` (`channels/types.rs`): add `planner: Option<AgentId>` to `ChannelPost` and `NewPost`, with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Update the few literal constructors.
- `aura-ui` (`views/channels.rs`): add `planner: Option<String>` (hex) to `ChannelPostView`, with a serde default.
- `aura-app`:
  - `start_planner` appends a user post with the text, `mentions: [planner]` and `planner: Some(planner)` once the planner exists. This is only for non-empty text: `/plan` alone and a switch to plan mode store nothing.
  - It sends the post to the coordinator's UI like any post and asks for its summary through `summaries::spawn_for`, so long ones are summarized exactly like other posts.
  - `post_view` carries the planner.
  - `take_digest` leaves plan posts out, so the coordinator's turns read what they read today.
  - `send_posts_of` keeps them in the user's posts it returns, so their summaries load with the channel.
- `aura-ui-terminal`:
  - `set_channel_posts` and `add_channel_post` file plan posts in a separate `plan_posts` map, de-duplicated by `seq` like posts. `coordinator_posts`, and with it the chat, the empty-channel banner and `pane_is_empty`, never see them.
  - HISTORY builds an item of kind `HistoryKind::Plan { planner }` for each. These are merged by time with the posts and typed messages, newest first. Summaries are keyed by `seq`, as for posts. Plan items take no part in the post/typed de-duplication: a plan request never reaches the coordinator's chat.

## The jump

Enter or a click on a plan item opens the planner's own chat with `open_channel_member`, which is what a click on its MEMBERS row does. It then jumps to the planner's user message whose text matches the request, the one nearest the request's time, and highlights it with the existing flash.

`chat_jump.rs` generalizes the pending typed jump to name the chat it belongs to. It is either the channel's chat, as today, or a member's chat that must be on screen. The block is resolved once that chat holds the message, waiting up to `JUMP_WAIT` while the chat loads.

If the planner is no longer a member, a notice says so and nothing moves. Opening the planner's chat is the best fit because that is where the request and its plan live; the channel's chat never showed it.

## Tests

- **aura-store-db:** a post with a planner round-trips. An old row without the field reads with `planner: None`.
- **aura-app:** `/plan <text>` stores one user post naming the planner, sends it and asks for its summary. `/plan` with no text stores nothing. The coordinator's digest leaves plan posts out.
- **aura-ui-terminal listing:** a plan post is listed in HISTORY in time order among posts and typed messages, but is not drawn in the channel's chat.
- **aura-ui-terminal de-duplication:** the same plan post arriving live and again in a reload is listed once. Typed text equal to a plan request is not merged into it.
- **aura-ui-terminal summaries:** a summary by `seq` shows for a long plan request.
- **aura-ui-terminal jump:** Enter on a plan item opens the planner's chat and lands on and highlights its request. A planner that left the channel gets the notice.

## Shipping

The work goes on `aura/channel-history-tab-rus200`, fast-forwarded to origin/main (`9b459c29`), pushed without force, with a PR. Then I bring in the latest origin/main and rerun the tests for aura-store-db, aura-ui, aura-ui-terminal, aura-desktop and aura-app, plus fmt, clippy on touched files and the file-size check. I merge with a normal merge commit. `49d582b9` and the stash stay local.
