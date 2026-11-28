# Performance with many agents

How the app holds up with many agents at once, how to measure it, and
what it is held to.

## The load harness

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

## Numbers

Release build, 4 vCPU (AMD EPYC), 10 s measured. Times are in
milliseconds.

### Before (`AURA_LOAD_UNPACED=1`, the behaviour before this change)

| N | loop busy | probe wait p99 / max | slowest kinds (total ms) | UI work per 10 s | frame p99 | longest UI stall |
|---|---|---|---|---|---|---|
| 10 | 1% | 0.03 / 0.04 | inbox message 43, inbox save 22, work done 11 | ≈ 20 ms | 1.8 | 2.7 |
| 50 | 7% | 0.04 / 0.06 | inbox message 347, inbox save 179, work done 144 | ≈ 180 ms | 1.5 | 2.1 |
| 200 | **68%** | **15.6 / 32.0** | inbox message 2,648, work done 2,536, inbox save 1,574 | ≈ 1,860 ms | 1.3 | 12.4 |

At N = 200, every loop event that started or ended a turn cost 2 to 17
ms, and the UI processed 2,500 copies of each agent list per 10 s.

### After

| N | loop busy | probe wait p99 / max | slowest kinds (total ms) | UI work per 10 s | frame p99 | longest UI stall |
|---|---|---|---|---|---|---|
| 10 | 1% | 0.03 / 0.04 | inbox message 33, agent lists 11, inbox save 3 | ≈ 20 ms | 1.7 | 1.8 |
| 50 | 3% | 0.04 / 0.06 | inbox message 219, agent lists 16, work done 12, inbox save 9 | ≈ 140 ms | 1.4 | 1.6 |
| 200 | **15%** | **0.05 / 0.08** | inbox message 962, work done 489, inbox save 27, agent lists 9 | ≈ 1,000 ms | 2.9 | 12.4 |

## What held the loop up, ranked

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

## The fixes

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

## What it is held to

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

## Not covered yet

- **Real cloud sockets.** The harness feeds cloud streams in-process, so
  the cloud hub's sockets, mirror copies and locks are not measured.
- **The INBOX store** rebuilds its rows on every mail. It is capped at
  2,000 messages, so this costs at most about 2 ms per mail once full.
- **Memory** grows with the load: resident 100 to 290 MiB at N = 200
  over 10 s. That is the memory side's work.
