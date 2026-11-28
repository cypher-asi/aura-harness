---
name: Progressive fast startup
overview: Measure where startup time goes with 10, 50 and 200 agents, then make the current chat usable first. The UI draws at once, the event loop takes input before startup work is done, and the rest loads in the background in priority order. Every restart guarantee is kept.
todos:
- id: measure
  content: Add startup_perf harness (N=10/50/200) and record before numbers
  status: pending
- id: open-on
  content: SessionOptions::open_on from ui_selection.json; open on remembered chat
  status: pending
- id: opening-order
  content: Cheap list first, current chat, then full rows in show_opening
  status: pending
- id: startup-steps
  content: run_loop starts after restore_saved; StartupSteps drained below UI events, StartupDone
  status: pending
- id: ui-first
  content: CLI and desktop draw UI before session; App::attach_session and placeholders
  status: pending
- id: transcript
  content: Tail-only transcript load at startup, or leave to RUS201
  status: pending
- id: tests
  content: 'Tests: chat first, open_on, guarantees, placeholders'
  status: pending
- id: docs
  content: After numbers in docs/performance.md, STYLEGUIDE, changelog fragment
  status: pending
- id: ship
  content: fmt, clippy, tests; branch off main, PR, merge with --match-head-commit
  status: pending
---

# Progressive fast startup

Measure where startup time goes with 10, 50 and 200 agents, then make the current chat usable first. The UI draws at once, the event loop takes input before startup work is done, and the rest loads in the background in priority order. Every restart guarantee is kept.

## Todos

- [ ] Add startup_perf harness (N=10/50/200) and record before numbers
- [ ] SessionOptions::open_on from ui_selection.json; open on remembered chat
- [ ] Cheap list first, current chat, then full rows in show_opening
- [ ] run_loop starts after restore_saved; StartupSteps drained below UI events, StartupDone
- [ ] CLI and desktop draw UI before session; App::attach_session and placeholders
- [ ] Tail-only transcript load at startup, or leave to RUS201
- [ ] Tests: chat first, open_on, guarantees, placeholders
- [ ] After numbers in docs/performance.md, STYLEGUIDE, changelog fragment
- [ ] fmt, clippy, tests; branch off main, PR, merge with --match-head-commit

<!-- end of generated header: edit below -->

## How startup works today

Both `aura-cli` (`runner.rs`) and `aura-desktop` (`lib.rs`) await `aura_app::session::start` before they create the UI. Nothing is drawn until that returns.

`session::start` (`crates/surface/aura-app/src/session/mod.rs`) does this work in order:

- It opens the store (`local_store::open`, which includes #93's encryption migration) and the registry.
- It archives spawns that never started, a store read per spawned agent.
- It reads the active agent's records and rebuilds its whole chat (`conversation_history`).
- It builds the hub.
- It runs `show_opening`, which builds every agent's list row (a store read per agent) and channel summaries, then sends the active chat.

The session opens on `registry.active()`, not on the channel or agent the user was last in. The UI restores #53's remembered selection only after start returns, and then switches. So a user who was in a channel sees a different chat first and waits for a second load.

`run_event_loop` (`event_loop/mod.rs`) then awaits three steps before `run_loop` handles a single UI event:

- `inbox::restore_saved` reads `inbox-owed.json` (#96) and the factory flows (#95).
- `send_startup` awaits `cloud::start`, which includes `ensure_hub`, a network connect, and the active agent's side panels.
- `inbox::resume_saved` refreshes peer reach and resumes every cut-short turn and owed report (#85, #96, #97).

Typing, switching chats and opening channels all queue behind that, which is the frozen screen the user sees.

## Phase 1: Measure first

An ignored test, `crates/surface/aura-app/tests/startup_perf.rs`, seeds a temporary data dir with N agents (10, 50 and 200). Each agent gets a realistic record. Some agents are channel members, a few are cloud-placed, and an `inbox-owed.json` holds owed reports and running turns.

The test runs `session::start` with the mock provider and times four points from the call:

- (a) start returning;
- (b) the active chat's `LoadConversation` being sent;
- (c) the loop answering a probe `UiEvent::RefreshAgents`, which counts as "current chat usable";
- (d) a new `UiCommand::StartupDone`, which counts as "fully loaded".

Timing spans log per phase so the numbers say where the time goes. I'll use RUS203's load harness if it covers startup, and the seeding above if not (asked). The before numbers go into `docs/performance.md` under a Startup section, shared with RUS203 and RUS201.

Phase 2 below is the default. If the measurements point elsewhere, for example to the store open, I'll aim the fix there and say so.

## Phase 2: Current chat first, then the rest

**Open on the chat the user was in.** `SessionOptions` gets `open_on: Option<OpenOn>`, either a channel id or an agent id. The CLI and the desktop read `ui_selection.json` before calling start, using a new `session::data_dir(&options)` to find it. The session then opens on the remembered channel's coordinator or the remembered agent, so the first chat sent is the right one and nothing reloads.

**Show the UI before the session is up.** The CLI and the desktop create the `App` or window first and start the session in the background, handing its channels over with a new `App::attach_session`. Until then the chat body says "Opening your workspace…" and the lists show a placeholder row. Both a blank screen and a frozen one are gone.

**Send in priority order.** `show_opening` keeps the desktop's rule that a tab learns its agent before that agent's chat.

- It first sends a cheap list (`SetAgents`, `SetChannels` and `SetActiveAgent`). The cheap list is built from the registry alone, without each agent's store read.
- Then it sends the current chat.
- The full list rows, with last activity, come afterwards from the loop's startup steps.

**Take input before startup work.** `run_loop` starts right after `restore_saved`. That restore is a local file read, and it stays first so restored queues and owed reports keep their place ahead of anything the user does next (#96).

Everything else becomes ordered `StartupStep`s, fed through a channel that `run_loop` drains with lower priority than UI events. A step runs only when no UI event is waiting. The steps run in this order:

1. The current agent's side panels: sandbox, HUD, mode, policies, instructions and plan.
2. Then the full agent rows and channel summaries.
3. Then `cloud::start`.
4. Then `resume_saved`.
5. Then models and traces.
6. Then `StartupDone`.

Resume therefore still happens, and in the same order relative to the restore. It is just no longer in front of the first frame or of input.

**Load old transcript parts on demand.** At startup the session already loads only the current chat. Rebuilding it reads the whole record, so it should load just the newest part and fetch older parts as the user scrolls up. I've asked RUS201, who is working on lazy chat loading, whether they're building that. If they are, I leave it to them. If not, I add a tail-only `conversation_history` for startup in `record_loader`, leaving `agent_views.rs` and `view_cache.rs`, their files, alone.

**History summaries.** These are already spawned per channel when its posts load, so they stay off the startup path.

## Boundaries

- **Cloud start and resume (RUS197).** I only change where `cloud::start` and resume are awaited from, not what they do inside. I've asked RUS197, who is working on cloud reconnect at startup, which files they're editing.
- **Panels (RUS206, FEN6).** I touch only the empty-state text of the lists and the chat, and will message them before editing panel files.
- **Restart guarantees.** The existing tests for #85, #95, #96 and #97 must pass unchanged.

## Tests

- **Chat first.** The current chat's `LoadConversation`, and the loop's answer to a UI event, both arrive before `StartupDone` while slow startup steps are held back. A stub step blocks until the test releases it.
- **Right chat.** With `open_on` set to a channel, the first chat sent is its coordinator's.
- **Guarantees kept.** Restored owed reports run first and restored queues keep their order. A cut-short turn resumes once, after the first UI event has been answered.
- **Placeholders.** The terminal shows the loading placeholders before attach, and the right chat after.
- **Numbers.** Before and after timings at N = 10, 50 and 200 go into `docs/performance.md`, with a changelog fragment.
