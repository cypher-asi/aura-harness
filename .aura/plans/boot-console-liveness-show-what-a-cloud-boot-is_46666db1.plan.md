---
name: 'Boot console: clickable fold, and show what a cloud boot is waiting on'
overview: Fix the finished boot console so that "click to show the boot log" really opens it. Then make every slow wait in a cloud move or wake say what it is waiting on and when it gives up, so a stuck boot and a slow one look different.
todos:
- id: fold-rows
  content: Return fold rows (header, summary, hint) from boot_lines in renderer/boot.rs
  status: completed
- id: fold-toggles
  content: Push a Step toggle for every fold row in single_lines (chat_lines.rs)
  status: completed
- id: fold-tests
  content: Add app click test and renderer fold-rows test; run fmt, clippy, aura-ui-terminal tests
  status: completed
- id: boot-wait-event
  content: Add BootStage::Connect, BootEvent::Waiting, BootReporter::wait and tracing in run (boot.rs) with tests
  status: completed
  assignee: rust-engineer
- id: fleet-waits
  content: Wrap quota/create/configure/claim/wake in boot.wait in fleet.rs
  status: completed
- id: snapshot-waits
  content: Report builder create, each step, uploads and snapshot-ready polling in snapshots.rs
  status: completed
- id: upload-progress
  content: 'Optional: VmProvider::upload_reported with byte progress in boxd.rs'
  status: completed
- id: hub-waits
  content: Add waits in hub.rs (plans, delta pack/upload, node import) and start_reported for adopt/sweep
  status: completed
- id: app-mapping
  content: Map Waiting in event_loop/cloud.rs, route connect through a reporter, skip Waiting in chat_notes.rs
  status: completed
- id: ui-model
  content: Add BootUpdate::Waiting, BootStep.waiting, BootConsole.last_update and transcript line
  status: completed
- id: ui-render
  content: Render the waiting sub-line with limit colouring and the 'no news for' header label
  status: completed
- id: tests-gates
  content: Add phase 2 tests and run fmt, clippy -D warnings and tests for the three crates
  status: completed
---

# Boot console: clickable fold, and show what a cloud boot is waiting on

Fix the finished boot console so that "click to show the boot log" really opens it. Then make every slow wait in a cloud move or wake say what it is waiting on and when it gives up, so a stuck boot and a slow one look different.

## Todos

- [x] Return fold rows (header, summary, hint) from boot_lines in renderer/boot.rs
- [x] Push a Step toggle for every fold row in single_lines (chat_lines.rs)
- [x] Add app click test and renderer fold-rows test; run fmt, clippy, aura-ui-terminal tests
- [x] Add BootStage::Connect, BootEvent::Waiting, BootReporter::wait and tracing in run (boot.rs) with tests (@rust-engineer)
- [x] Wrap quota/create/configure/claim/wake in boot.wait in fleet.rs
- [x] Report builder create, each step, uploads and snapshot-ready polling in snapshots.rs
- [x] Optional: VmProvider::upload_reported with byte progress in boxd.rs
- [x] Add waits in hub.rs (plans, delta pack/upload, node import) and start_reported for adopt/sweep
- [x] Map Waiting in event_loop/cloud.rs, route connect through a reporter, skip Waiting in chat_notes.rs
- [x] Add BootUpdate::Waiting, BootStep.waiting, BootConsole.last_update and transcript line
- [x] Render the waiting sub-line with limit colouring and the 'no news for' header label
- [x] Add phase 2 tests and run fmt, clippy -D warnings and tests for the three crates

<!-- end of generated header: edit below -->

## Phase 1: the "click to show the boot log" row does nothing

Once a move ends, the console folds to its header, the outcome summary and a faint `click to show the boot log` row. Only line 0, the header, is registered as clickable. Clicking the hint or the summary finds no toggle row, so nothing happens.

The relevant code is in `single_lines` in `crates/ui/aura-ui-terminal/src/renderer/chat_lines.rs`:

```rust
let toggles = message
    .boot_console()
    .filter(|console| !console.is_running())
    .map(|_| ToggleRow { line: 0, target: ToggleTarget::Step(message.version().0), open: message.is_expanded() })
    .into_iter()
    .chain(more_toggles(message, more, 0))
    .collect();
```

`toggle_row_at` in `app/mod.rs` only matches exact lines. The header click works, though only off the `⧉ Copy` button, because a press on the button copies instead. The rows below it never had a toggle.

The fix:

- `boot_lines` in `renderer/boot.rs` returns the rows that fold the console, along with its lines and more-rows. For a folded console that is the header, every summary row and the hint row. For an open console it is the header and the summary rows, so clicking the same place closes it again.
- `single_lines` pushes a `ToggleRow` with `ToggleTarget::Step(id)` for each of those rows, instead of only line 0. Hover comes for free, because `hovered_activity` uses the same toggle rows, so the rows now highlight as clickable.
- The copy button keeps priority. It is handled on mouse-down in `copy_code_at`, which returns before a selection starts, so a click on it never toggles.
- The earlier-lines `MoreSection::BootLog` row lives in the log section, below the fold rows, so the two never share a line.

Tests:

- An app test in `app/tests.rs`, in the style of `clicking_the_earlier_lines_row_of_a_boot_console_loads_them`. It draws a finished console, clicks the `click to show the boot log` cell and checks that the message is expanded and a log line is on screen. It then clicks the header and checks that the console folds again.
- A renderer test in `renderer/boot.rs` that the fold rows returned cover the header, the summary and the hint when folded, and the header and summary when open.

## Phase 2: why a boot looks stalled today

While a move runs, the console (`BootConsoleUi` in `crates/surface/aura-app/src/event_loop/cloud.rs`) shows the `BootReporter` events from `crates/engine/aura-engine-cloud/src/boot.rs`, with a spinner and a timer on the running step. Several steps make long remote calls that send no events at all:

| Step | What runs silently | Time limit today |
|---|---|---|
| Connecting to boxd | `CloudHub::start`: a `GetVm` for each placed agent, then `sweep_orphans`, awaited inline on the event loop | 30s per call |
| Preparing project snapshot | the builder `provider.create`, `cloud-init status --wait` (output sent to `/dev/null`), the uploads, the save-snapshot poll loop | 3m, 90m, 3m, 10m |
| Creating machine | `CreateVmFromSnapshot` / `ForkVm` | 3m |
| Claiming aura node | the claim exec, whose output is never streamed by design | 60s |
| Plans, Configure, Wake | single RPCs | 30s |

Everything has a time limit, but the user can't see what is pending or when it gives up. Your 14-minute move most likely spent most of that time in the snapshot build.

## Phase 2: the approach

There are two signals:

- **What it's waiting on (engine side).** A new `BootEvent::Waiting` names the pending call and its time limit. The running step shows it as a sub-line, for example `↳ boxd is creating aura-re36-3f9c from snapshot proj-x · 42s / 3m 00s`. The line turns to warning colour near the limit and says "past its time limit" once the limit goes by.
- **When it last heard anything (UI side).** The console records when it last got an update. After about 15 quiet seconds the header adds `· no news for 47s`, in warning colour after 60s.

### Engine

In `boot.rs`:

- Add `BootStage::Connect`.
- Add `BootEvent::Waiting { stage, what: Option<String>, limit: Option<Duration> }`.
- Add `BootReporter::wait(stage, what, limit, fut)`. It sets the waiting line, awaits the work, then clears the line whatever the outcome.
- Have `BootReporter::run` log each step's start, its finish with milliseconds, and any failure through `tracing`.

In `fleet.rs`:

- Wrap quota, create, the configure RPCs, claim and wake in `boot.wait`, each with its timeout constant.

In `snapshots.rs`:

- Wrap the builder create, each `Step::Run` and each upload in waits. Give the cloud-init step a readable label.
- In the save-snapshot poll loop, update the waiting line on every poll with boxd's status and the time left.

Optional:

- Add `VmProvider::upload_reported`. It would report bytes sent as `boot.progress`, defaulting to plain `upload`.

In `crates/surface/aura-app/src/cloud/hub.rs`:

- Wrap the plan sync, the delta pack and upload, and the node `head`/`import` calls in waits.
- Add `start_reported(keep, boot)` that reports each `fleet.adopt` and `sweep_orphans`. `start` calls it with `BootReporter::none()`.

### App

In `event_loop/cloud.rs`:

- Map `BootEvent::Waiting` to `BootUpdate::Waiting { key, what, limit_ms }`.
- Route `connect` through a reporter under `BootStage::Connect`, replacing its hand-built stage updates.

In `chat_notes.rs`:

- Drop `Waiting`, as `Progress` is dropped, so it is never replayed.

### Terminal UI

- Add `BootUpdate::Waiting` in `crates/ui/aura-ui/src/events.rs`.
- In `components/boot.rs`, give `BootStep` a `waiting` field that is set, replaced and cleared, and also cleared on leaving `Running` and on `End`. Give `BootConsole` a `last_update` with a `quiet_ms()` accessor. Have `transcript()` include the waiting line.
- In `renderer/boot.rs`, have `step_lines` draw the `↳` sub-line with colouring by limit. Have `header` add `no news for …` while the console is running.

### Tests

- Engine tests for `wait`, and a `FakeProvider` test for the waits around create and claim.
- UI model and renderer tests for the waiting sub-line and the quiet label.
- A `boot_updates` mapping test, and a `chat_notes` test that `Waiting` is not persisted.

## Checks

Run these after each phase:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test -p aura-ui-terminal`, plus `-p aura-engine-cloud -p aura-app` for phase 2

## Left out on purpose

- There is still no way to cancel a move mid-boot.
- `ensure_hub` still runs on the event loop.

Both are sensible follow-ups.
