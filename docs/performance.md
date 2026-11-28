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
   and each finished subagent, keeps its model conversation and kernel
   for the rest of the run, archived agents included. That is about as
   large as the agent's chat, up to its context window. It is not fixed
   yet: it is in the event loop, which is being reworked for
   responsiveness.
3. **Smaller caches**: the HISTORY tab's summaries in memory are now
   capped at 2,000 beside the channel store, and the trace step builder
   clears its cache-hit and blocked tool ids each turn. Already bounded,
   and left alone: the TRACE feed (300), the shown chats and records read
   ahead (8), mail (2,000) and the flow graph.

`an_idle_agent_off_screen_holds_little_however_long_its_chat` and
`a_long_session_grows_with_the_chats_kept_not_with_the_agents` run with
the tests and fail if an idle agent off screen holds more than 64 KiB.
