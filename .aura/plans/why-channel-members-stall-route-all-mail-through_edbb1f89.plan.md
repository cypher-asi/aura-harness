---
name: 'Why channel members stall: route all mail through the inbox, never run an agent twice, make starts visible and reported'
overview: 'The three symptoms come from two gaps: the hub only knows about the turns and mail it handles itself, and the app has no idea what a member is waiting on. The fix routes every member request through the app''s inbox, has the node turn away a second run for an agent that is already busy (the hub waits instead), shows the coordinator why a member is idle, and reports any start that fails or stalls.'
todos:
- id: hygiene
  content: Stash the foreign WIP (git stash -u) and fast-forward to origin/main
  status: pending
- id: desk-spawned
  content: desk_for and MachineMail file requests from agents spawned on a member's machine
  status: pending
- id: desk-refused
  content: Add MailDesk::refused and notify the sender on every undelivered request
  status: pending
- id: reach-refresh
  content: Add inbox::refresh_reach; call it on hub start, after assign, and on the sweep
  status: pending
- id: node-busy
  content: 'Node: 409 agent_busy for a second chat run of an agent with a turn under way'
  status: pending
- id: hub-wait
  content: 'Hub: on agent_busy, wait and retry start_run instead of starting a duplicate'
  status: pending
- id: standing
  content: Add AgentHub::standing and a state/why/since_ms field in peer_row
  status: pending
- id: start-reports
  content: Report slow or failed new-member moves, and give up work that can't start
  status: pending
- id: sweep
  content: 'Add a once-a-minute stall sweep with a parked set: retry once, then report'
  status: pending
- id: tests
  content: Write a failing-first test for each fix; run fmt, clippy, tests and the file-size check
  status: pending
- id: ship
  content: Commit, push, open the PR, merge latest main, retest, merge with a merge commit, report
  status: pending
---

# Why channel members stall: route all mail through the inbox, never run an agent twice, make starts visible and reported

The three symptoms come from two gaps: the hub only knows about the turns and mail it handles itself, and the app has no idea what a member is waiting on. The fix routes every member request through the app's inbox, has the node turn away a second run for an agent that is already busy (the hub waits instead), shows the coordinator why a member is idle, and reports any start that fails or stalls.

## Todos

- [ ] Stash the foreign WIP (git stash -u) and fast-forward to origin/main
- [ ] desk_for and MachineMail file requests from agents spawned on a member's machine
- [ ] Add MailDesk::refused and notify the sender on every undelivered request
- [ ] Add inbox::refresh_reach; call it on hub start, after assign, and on the sweep
- [ ] Node: 409 agent_busy for a second chat run of an agent with a turn under way
- [ ] Hub: on agent_busy, wait and retry start_run instead of starting a duplicate
- [ ] Add AgentHub::standing and a state/why/since_ms field in peer_row
- [ ] Report slow or failed new-member moves, and give up work that can't start
- [ ] Add a once-a-minute stall sweep with a parked set: retry once, then report
- [ ] Write a failing-first test for each fix; run fmt, clippy, tests and the file-size check
- [ ] Commit, push, open the PR, merge latest main, retest, merge with a merge commit, report

<!-- end of generated header: edit below -->

## Before anything: the workspace has someone else's changes in it

This clone came with 71 uncommitted modified files and 4 untracked ones. Among them is an unmerged `hub/follow.rs` and `hub/marks.rs`, which reattach to a cloud turn whose stream dropped. This looks like in-progress work copied from the user's project folder. `git_ship` stages everything, so I will first set it aside with `git stash push -u -m "foreign WIP found in the workspace snapshot"`, which keeps it recoverable. Then I fast-forward to `origin/main` (HEAD is an ancestor of it). All the analysis below is against `origin/main`. The report will warn whoever owns that work: it touches `hub/turn.rs`, `hub/links.rs` and `hub/node_client.rs`, which this fix touches too.

## Root cause A: mail can bypass the app inbox (problems 2 and 3)

Mail between channel members is sent up to the hub (`MeshRouter::route` in `aura-node/.../handlers/cloud/mod.rs`). The hub files a request at the app's inbox (`MachineMail`) only when `CloudHub::desk_for` (`hub/reach.rs:106`) finds the recipient in the sender's reach. That reach has three gaps:

- It is set only when the sender's own turn starts (`cloud/submit.rs:114`).
- It lives only in memory, so it is empty after a hub restart.
- It is keyed by the placed agent, so an agent spawned on that machine (a reviewer's subagent, for example) never has one.

Any other mail takes the `elsewhere` branch of `take_mail` (`hub/links.rs:104-115`). That branch forwards it straight to the recipient's node, where `HostInner::deliver_to_root` (`aura-engine/.../host/messaging.rs:160`) does one of two things:

- If the recipient's last chat run is still open, it drops the mail into that run's mailbox. The run's idle loop (`chat/idle.rs:37-48`) then starts a turn on the machine that the app never hears about. The app still shows the agent as idle, so the next assignment or message runs as a second session of the same agent, on the same branch and worktree. That is FEN6's duplicate.
- If that run has closed, it only records the mail on the agent's log (`record_message`). The agent never sees it, and neither does the INBOX tab. That is REV69's lost re-review.

When the inbox does refuse a request (full thread, unknown agent), the hub acks it as undelivered. Nodes ignore acks, so the sender is never told (see the failure modes note in `reach.rs`).

The fix:

- **Spawned agents' requests are filed too.** `desk_for` looks up the sender's placed root (`machine_root`, children), so an agent spawned on a member's machine uses its root's reach. `MachineMail::file` files the request as from that root and names the subagent in the text.
- **Reach is kept current.** A new `inbox::refresh_reach` computes `reach_of` for every placed channel member and calls `set_reach`, which does nothing when the reach is unchanged. It runs when the hub starts (`cloud::start`, `connect`), after `assign` adds a member, and on the stall sweep below.
- **Refusals reach the sender.** `MailDesk` gets a `refused(&RemoteMail, &str)` method with a default no-op, called on every undelivered path of `take_mail` where the sender is known. `MachineMail` implements it as a harness notice to the sender: "Your message to X about S was not delivered: why". A refusal inside `MachineMail::file` does the same.

## Root cause B: nothing stops a second run for an agent that is already busy (problem 2)

`POST /v1/run` (`aura-node/.../handlers/run/mod.rs:84`) starts a new chat run even when another run of the same agent has a turn under way, which `ChatRunHandle::activity` records. The hub starts a new run for every turn (`stream_turn` in `hub/turn.rs`) and only knows the turns it streams itself. Three kinds of turn look idle to the app even though they are running:

- A turn the node started from mail (root cause A).
- A turn whose event stream dropped. On main, `relay` treats a closed stream as the end of the turn while the node keeps running it.
- A turn asked for before the app restarted.

The fix:

- **The node refuses the second run.** If another top-level chat run of the same agent has a turn under way, the node answers `409` with `{"code": "agent_busy", "run_id": …}`.
- **The hub waits instead of failing.** `NodeClient::start_run` maps that answer to a busy result. `stream_turn` then shows "Waiting for the turn already running on its machine" and retries every few seconds until that turn ends, within the existing `TURN_TIMEOUT`, stopping if the user cancels. Only then does it start the run.

So a message to a busy member waits behind the work already running on that member's machine, and never starts a duplicate. Reattaching to a dropped stream is what the uncommitted `follow.rs` does, so I am not doing that here.

## Root cause C: a new member's start is invisible and unreported (problem 1)

Here is what happens when the coordinator starts a new member:

- `assign` starts the member, and `deliver` calls `start_in_cloud`, which spawns `hub.place` (`cloud/placement.rs:166-201`) with no deadline and no one watching it.
- While the move runs, the assignment waits in the queue (`cloud/submit.rs:24-28`). `is_working` (`subagents/running.rs:275`) leaves out moves and queues, so `get_agent_state` shows `working: false` (`inbox/hooks.rs:18-26`) with no reason.
- The base snapshot's fingerprint includes the local binary's digest (`snapshots/ensure.rs`). So the first placement after the app is rebuilt rebuilds the base and the project snapshots, one build at a time (`snapshots/mod.rs:147`), with up to 90 minutes per step (`STEP_TIMEOUT`).
- A move that fails is reported only in the member's own chat (`placement.rs:189-197`).

Several paths also drop the work without telling anyone:

- `take_session` failing (`start_work.rs:31`).
- `local_model_unavailable` (`start_work.rs:150`).
- The refusals in cloud `submit`: `spawned_on_machine`, `foreign_owner`, `plan_refusal`, and the bare `hub()?`.

The member keeps owing its report, and nothing ever looks at it again.

The fix:

- **The coordinator can see why a member is idle.** A new `AgentHub::standing` map, written by the loop, holds one of: starting in the cloud (since when, and the boot stage reached, taken from the move's console events), queued (how many), or failed to start (why). `peer_row` adds `"state"` to `list_agents` and `get_agent_state`: `working`, `starting in the cloud`, `queued`, `idle`, `failed to start` or `stalled`, plus `"why"` and `"since_ms"` when they apply.
- **Slow and failed starts are reported.**
  - When a new member's move passes 15 minutes, each agent it owes gets one notice naming the boot stage it is in.
  - When the move fails, they get a notice that it works on this computer instead.
  - When work can't start at all, a new `inbox::give_up_unstarted` gives up everything the agent owes, with the reason, and marks it "failed to start". It does this only if nothing else is running or queued for that agent.
- **A stall sweep runs once a minute** (a tick in `run_loop`). It looks for an agent that owes reports and is not working, not moving and has nothing queued. It skips agents the user stopped on purpose: an `Inbox::parked` set is filled where PR #60 keeps owed reports after a user stop, and cleared when the agent's next work starts. An agent seen stalled on two sweeps in a row is retried once, with a carry-on prompt that lists what it owes. If it is still stalled after that, its reports are given up with "went idle without finishing".

I will not cancel a slow move. Dropping a snapshot build part-way could leave a builder machine behind, and the step timeouts already bound how long a move can take.

## What this does not fix

The inbox's owed list, the work queues and the reach all live in memory. A restart of the app still forgets what members owe, so the sweep can't find those reports afterwards. Fixing that means persisting the inbox, which is a design decision I will raise in the report. A turn whose stream dropped is still settled as failed on main, until the reattach work in `follow.rs` lands. With this fix it no longer causes a duplicate run.

## Tests, each failing without its fix

| Crate | Test |
|---|---|
| `aura-engine-cloud` (`hub/tests/mail.rs`) | A request from an agent spawned on a placed member's machine is filed at the desk, not forwarded to the recipient's node. |
| `aura-engine-cloud` (`hub/tests/mail.rs`) | A refused request reaches `MailDesk::refused`. |
| `aura-engine-cloud` (`hub/tests/wire.rs` or a new file) | `start_run` sees `409 agent_busy`, waits, then starts the run once the node accepts it. |
| `aura-node` (gateway tests) | A second `POST /v1/run` for an agent with a turn under way gets `409 agent_busy`. It succeeds once that turn ends, and runs of other agents are unaffected. |
| `aura-app` (inbox tests) | A refused mail sends a notice to its sender. A spawned agent's request is filed as from its root. |
| `aura-app` (inbox tests) | `get_agent_state` shows `starting in the cloud` with its stage, `queued` and `failed to start`. |
| `aura-app` (inbox tests) | Work that can't start gives up what the agent owes, and the assigner gets a report. |
| `aura-app` (inbox tests) | The sweep retries a stalled member once, then reports it. It leaves parked and working members alone. |
| `aura-app` (inbox tests) | A failed or slow start of a new member tells the assigner. |

The gate is `cargo test -p aura-app -p aura-engine-cloud -p aura-node`, `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` on those crates, and `bash scripts/check_file_size.sh`. New code goes in new modules where a file is near the 500-line limit (`cloud/mod.rs` is at 488, `hub/links.rs` at 439).

## Shipping

I will commit on `aura/why-members-stall-rus202` off the latest `origin/main`, push without force and open a PR with `gh pr create`. Then I merge in the latest `origin/main`, rerun the tests and merge to main with a normal merge commit, unless a conflict would change behavior. The report lists each root cause with file and line, the PR link, the merge commit, the stashed changes, and what is still open.
