# Performance

## Memory

What the terminal UI keeps per agent, measured with dhat's heap profiler
in `crates/ui/aura-ui-terminal/tests/ui_memory.rs`:

```sh
cargo test --release -p aura-ui-terminal --test ui_memory -- --ignored --nocapture
```

Each agent takes 20 turns, each turn a 2 KiB reply and a `write_file`
call with 20 KiB of arguments and a 20 KiB result. The session's 300
Record panel rows come with each switch. Every agent is shown once, then
the first is shown again. "Held" is the heap the UI holds then, over an
empty UI's. `AURA_MEMORY_AGENTS` and `AURA_MEMORY_TURNS` change the
counts.

| Agents | Before: held | Before: per agent | After: held | After: per agent |
|-------:|-------------:|------------------:|------------:|-----------------:|
| 10     | 13.5 MiB     | 1,377 KiB         | 7.2 MiB     | 736 KiB          |
| 50     | 63.6 MiB     | 1,302 KiB         | 7.4 MiB     | 152 KiB          |
| 200    | 251.5 MiB    | 1,288 KiB         | 8.3 MiB     | 43 KiB           |

Over a long session (12 agents, six rounds of five turns each, one agent
shown per round), the heap after each round:

| Round        | 1   | 2   | 3    | 4    | 5    | 6    |
|--------------|----:|----:|-----:|-----:|-----:|-----:|
| Before (MiB) | 4.7 | 8.3 | 12.3 | 15.9 | 20.1 | 23.5 |
| After (MiB)  | 1.5 | 2.4 | 4.1  | 6.2  | 7.3  | 8.3  |

After the fix, 36 agents hold within 0.2 MiB of 12. What still grows is
the chats kept whole, which grow with their agents' turns up to
`MAX_MESSAGES`, not with the number of agents.

### What held it

Ranked by size, before the fix:

1. **Off-screen chat views** (`app/agent_views.rs`): one per agent ever
   shown or heard from, never let go of. Each held up to 2,000 messages
   with every tool call's arguments and result, the laid-out lines for
   them, and the Record panel's rows. This was nearly all of the UI's
   per-agent memory. Now only the `KEPT_VIEWS` (4) agents most recently
   on screen, and an open channel's coordinator, keep theirs. Every
   other view lets go of its chat once nothing is under way, and is
   loaded again from the record when shown (`app/agent_views/shed.rs`).
2. **Idle sessions in the app** (`LoopState::idle`): each agent opened,
   and each finished subagent, kept its model conversation and kernel
   for the rest of the run, archived agents included: up to its context
   window per agent. Now the loop keeps the `IDLE_SESSIONS_KEPT` (16)
   sessions most recently left idle, and saves and lets go of older
   settled ones; an archived agent's go at once
   (`event_loop/idle_sessions.rs`). They reopen from their checkpoint.
   The load harness (below) shows no change in resident memory, because
   its members' conversations are a few short mock turns; what it saves
   in real use grows with how long each agent's conversation is.
3. **Smaller caches**: the HISTORY tab's summaries in memory are now
   capped at 2,000 beside the channel store, and the trace step builder
   clears its cache-hit and blocked tool ids each turn. Already bounded,
   and left alone: the TRACE feed (300), the shown chats and records read
   ahead (8), mail (2,000) and the flow graph.

The load harness's resident memory (`/proc/self/statm`), each N in its
own process, before and after the idle-session cap:

| Members | Before    | After     |
|--------:|----------:|----------:|
| 10      | 62 MiB    | 62 MiB    |
| 50      | 91 MiB    | 97 MiB    |
| 200     | 263 MiB   | 263 MiB   |

At 200 members, most of that is the UI and the harness's own feeds, not
sessions. The 50-member gap is run-to-run noise.

`an_idle_agent_off_screen_holds_little_however_long_its_chat` and
`a_long_session_grows_with_the_chats_kept_not_with_the_agents` run with
the tests and fail if an idle agent off screen holds more than 64 KiB.

## Responsiveness

How the app holds up with many agents at once, how to measure it, and
what it is held to.

### The load harness

`crates/surface/aura-app/src/event_loop/tests/load/` runs the real event
loop (`run_loop`) and the real terminal UI (`aura_ui_terminal::App` and
its renderer on a 200×60 test screen, paced as `Terminal::run` paces
them) with N simulated agents:

- a channel's coordinator with N members: half on this computer, half on
  cloud machines, plus N archived agents in the registry, as a long-lived
  data directory has them;
- each local member is assigned work every 2 s, which its mock model
  answers, so turns start and end, reports go back to the coordinator as
  inbox mail, and the coordinator's turns post to the channel;
- each cloud member streams one long turn through the real cloud turn
  path: text every 20 ms, and a tool step every 25 texts;
- the user switches agents every second and scrolls;
- a probe goes to the loop every 50 ms, to time how long an event waits
  before the loop takes it.

Run it in release:

```sh
AURA_LOAD_AGENTS=10,50,200 cargo test --release -p aura-app --lib -- load:: --ignored --nocapture
```

- `AURA_LOAD_SECS` sets the measured span; the default is 10 s, after a
  2 s warm-up.
- `AURA_LOAD_UNPACED=1` builds the agent lists and saves the inbox after
  every event, as the app did before the pacing below, for comparison.
- Each N prints a table. It shows:
  - how long probes waited (event-loop latency);
  - the loop's time per event, by kind;
  - frame and tick time;
  - the UI's time per command, by kind;
  - how many commands waited for the UI;
  - the longest stall on the UI thread;
  - the process's resident memory before and after.

The app logs a warning whenever one loop event takes over 100 ms, naming
its kind (`The event loop was held up by one event`). Events over 20 ms
are logged at debug level.

### Numbers

Release build, 4 vCPU (AMD EPYC), 10 s measured. Times are in
milliseconds.

#### Before (`AURA_LOAD_UNPACED=1`, the behaviour before this change)

| N | loop busy | probe wait p99 / max | slowest kinds (total ms) | UI work per 10 s | frame p99 | longest UI stall |
|---|---|---|---|---|---|---|
| 10 | 1% | 0.03 / 0.04 | inbox message 43, inbox save 22, work done 11 | ≈ 20 ms | 1.8 | 2.7 |
| 50 | 7% | 0.04 / 0.06 | inbox message 347, inbox save 179, work done 144 | ≈ 180 ms | 1.5 | 2.1 |
| 200 | **68%** | **15.6 / 32.0** | inbox message 2,648, work done 2,536, inbox save 1,574 | ≈ 1,860 ms | 1.3 | 12.4 |

At N = 200, every loop event that started or ended a turn cost 2 to 17
ms, and the UI processed 2,500 copies of each agent list per 10 s.

#### After

| N | loop busy | probe wait p99 / max | slowest kinds (total ms) | UI work per 10 s | frame p99 | longest UI stall |
|---|---|---|---|---|---|---|
| 10 | 1% | 0.03 / 0.04 | inbox message 33, agent lists 11, inbox save 3 | ≈ 20 ms | 1.7 | 1.8 |
| 50 | 3% | 0.04 / 0.06 | inbox message 219, agent lists 16, work done 12, inbox save 9 | ≈ 140 ms | 1.4 | 1.6 |
| 200 | **15%** | **0.05 / 0.08** | inbox message 962, work done 489, inbox save 27, agent lists 9 | ≈ 1,000 ms | 2.9 | 12.4 |

### What held the loop up, ranked

Measured at N = 200 by switching each suspect off and running again.

1. **The agent lists, rebuilt on every turn start and end and every
   assignment** (`AgentHub::send_agent_list`, 34 call sites).
   - Each build read every agent's record head, the archived agents' too.
   - Each sent four whole lists that the UI rebuilt its trees from.
   - Switching it off took the loop from 68% to 32% busy and probe wait
     p99 from 15.6 ms to 0.05 ms.
   - The UI also spent about 1.2 s per 10 s on the list commands.
2. **The inbox save after every loop event** (#96).
   - Each event built and compared a full snapshot of what is owed,
     queued, running, the threads and the factory's stages, and waited
     for the write.
   - It cost 1,574 ms of loop time per 10 s, of which only about 30% was
     the write; the rest was building and comparing the snapshot.
3. **What is left is the work itself.** Delivering an inbox message
   (about 1 ms) and ending a turn (about 0.5 ms) grow only slowly with N.

These were checked and are not the cause in this load:
- **Rendering:** a frame takes under 3.2 ms at any N, with off-screen
  chats not wrapped.
- **Cloud streams:** about 48,000 text commands per 10 s cost the UI
  167 ms in total.
- **Cloud hub lock contention:** the streams here are fed in-process,
  so the real cloud hub's sockets and locks are not exercised; see
  below.

### The fixes

- **Pacing** (`event_loop/pacing.rs`).
  - The agent lists are built at most every 250 ms and the loop's own
    inbox saves happen at most every 200 ms.
  - The first ask after a quiet interval still runs at once.
  - An ask within the interval marks the work due, and the loop runs it
    once the interval is over, with the state as it is then. The last
    ask is never lost.
  - While the loop handles what the user just did (a UI event), lists go
    at once, so the user sees their own action immediately.
- **What is not paced.** Saves that something waits on are still
  written, and waited for, at once:
  - mail filed by `assign`, `send_to_agent` and cloud machines
    (`saved::persist`);
  - the loop's last save at shutdown (`flush_saved`).
  - A crash can lose at most the loop's own changes of the last 200 ms,
    never anything a sender was told was sent.

### What it is held to

- **Every test run** (counts, not times):
  - a burst of 100 list asks builds the lists once, and the last ask
    still goes from the loop once the interval is over;
  - 100 loop events save the inbox once.
- **`load::budget::the_app_stays_responsive_with_200_agents`** (ignored;
  run it in release with `--ignored`), at N = 200:
  - probe wait p99 within 5 ms (15.6 ms before, 0.07 ms after);
  - the loop busy at most 40% of the time (68% before, 15% after);
  - no loop event over 100 ms;
  - frame p99 within 16 ms.
  - With `AURA_LOAD_UNPACED=1` it fails, as the code before this change
    does.

### Not covered yet

- **Real cloud sockets.** The harness feeds cloud streams in-process, so
  the cloud hub's sockets, mirror copies and locks are not measured.
- **The INBOX store** rebuilds its rows on every mail. It is capped at
  2,000 messages, so this costs at most about 2 ms per mail once full.
- **Memory** grows with the load: resident 100 to 290 MiB at N = 200
  over 10 s. See Memory above.

## Startup

How long the app takes to start with many agents
(`crates/surface/aura-app/tests/startup_perf.rs`):

```text
cargo test --release -p aura-app --test startup_perf -- --ignored --nocapture
```

The test writes N agents with 400 turns each (about 2 KB per message)
to a fresh data directory, opens it once as the previous run would, then
times `session::start` with the mock provider. The cloud is not
configured. Each time is in milliseconds from the call:

- **start**: `session::start` returns. No frame is drawn before this.
- **chat**: the chat the session opens on is sent.
- **usable**: the loop answers the UI's first event. From here, typing,
  switching chats and opening channels are handled.
- **loaded**: startup's background steps are done (`StartupDone`).

The middle of three runs, on the build machine (release build):

| Agents | Before: start | chat | usable | After: start | chat | usable | loaded |
|-------:|-----:|-----:|-------:|-----:|-----:|-------:|-------:|
| 10     | 153  | 154  | 203    | 117  | 118  | 118    | 165    |
| 50     | 114  | 115  | 168    | 108  | 108  | 109    | 149    |
| 200    | 119  | 120  | 162    | 122  | 123  | 124    | 160    |

Both columns are on the same main (`1ef4d378`). Measured again after
merging the later main, "after" was 159/160/160/202 ms at 10 agents,
134/134/135/184 at 50 and 126/127/128/165 at 200: run-to-run noise
is about 40 ms, but "usable" stays within a few milliseconds of
"chat" in every run.

Before, the loop took no UI event until it had sent the side panels,
brought the cloud hub up, refreshed peer reach and resumed everything
the last run cut short, so "usable" came 35 to 55 ms after "chat"
with no cloud. Each of those steps scales with what it waits on: with
the cloud configured, its connect and the reach refresh are network
calls. Now the loop takes UI events once the saved inbox is restored,
and runs those steps one at a time between them (`event_loop::startup`).
So "usable" follows "chat" within a few milliseconds whatever the
steps cost, and they finish by "loaded".

The number of agents barely moves startup. The session reads only the
open chat's record. The agents panel's rows took 4 ms with 200 agents,
and rebuilding a 400-turn chat about 30 ms. What remains is opening the
database (about 100 ms). The first start after a lot was written pays
more for it: 0.7 s with 200 agents of 400 turns
(`AURA_PERF_FIRST_OPEN=1`).

The session also opens on the chat the UI was last in, a channel or an
agent (`SessionOptions::open_on`, read from `ui_selection.json`).
Before, it opened on the registry's active agent and the UI then
switched, loading a second chat.
