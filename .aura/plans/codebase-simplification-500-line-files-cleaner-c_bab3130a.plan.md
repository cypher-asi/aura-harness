---
name: 'Codebase simplification: 500-line files, 50-line functions, cleaner crates'
overview: Bring all 1,376 Rust files (471k lines) under 500 lines and all functions under 50 lines, remove duplicated helpers and dead code, cut needless dependencies between crates, and simplify the largest types. No behavior changes; each step is checked by fmt, clippy, tests, the invariant gate, a file-size gate and clippy's `too_many_lines` at 50.
todos:
- id: p0-baseline
  content: Land or stash WIP; record clippy, test, file-size and fn-length baseline
  status: completed
- id: p0-size-gate
  content: Add check_file_size.sh with shrinking allowlist and wire into CI
  status: completed
- id: p0-fn-gate
  content: Add clippy.toml with too-many-lines-threshold = 50; fix 3 gate test fns
  status: completed
- id: p0-unwrap-lint
  content: Enable unwrap_used/expect_used lints; fix desktop expects
  status: completed
  assignee: implement
- id: p0-stray-files
  content: Remove 1.blob, test.wav, index.html from the repo root
  status: completed
- id: p1-allows
  content: 'Remove redundant crate-wide #![allow] blocks and fix the findings'
  status: completed
  assignee: rust-engineer
- id: p1-dead-code
  content: Drop dead_code allows in aura-agent verify/planning/file_ops and delete unused code
  status: completed
  assignee: rust-engineer
- id: p1-clippy-deny
  content: Switch CI clippy to -D warnings and update development.md
  status: completed
- id: p2-helpers
  content: Consolidate now_ms, truncation, formatting, console block and strip_ansi helpers
  status: completed
  assignee: rust-engineer
- id: p3-app-ui
  content: Point aura-app at aura-ui instead of aura-ui-terminal
  status: completed
- id: p3-app-node
  content: Move check_bearer and files_api down; drop aura-app's aura-node dependency
  status: completed
  assignee: rust-engineer
- id: p3-plugin-merge
  content: Fold plugin-api, plugin-connectors, plugin-mcp into aura-plugin
  status: completed
  assignee: rust-engineer
- id: p3-registry
  content: Extract agent_registry and agent_definitions into aura-engine-registry
  status: completed
  assignee: rust-engineer
- id: p3-cloud-hub
  content: Move the cloud hub into aura-engine-cloud; keep host_info in aura-app
  status: completed
  assignee: rust-engineer
- id: p3-widgets
  content: Extract aura-ui-terminal leaf modules into aura-ui-widgets
  status: completed
  assignee: rust-engineer
- id: p4-core-store
  content: 'Files and functions pass: core, config, store'
  status: completed
  assignee: rust-engineer
- id: p4-model-context
  content: 'Files and functions pass: model, context, plugin'
  status: completed
  assignee: rust-engineer
- id: p4-exec-agent
  content: 'Files and functions pass: exec and agent crates; DomainError'
  status: completed
  assignee: rust-engineer
- id: p4-auth-engine
  content: 'Files and functions pass: auth and engine crates'
  status: pending
  assignee: rust-engineer
- id: p5-app-state
  content: Group App's 158 fields into ListState and sub-state structs
  status: completed
- id: p4-ui
  content: 'Files and functions pass: aura-ui and aura-ui-terminal'
  status: completed
- id: p4-surface
  content: 'Files and functions pass: app, node, cli, desktop, root tests'
  status: in_progress
- id: p5-errors
  content: Replace remaining anyhow in library crates with thiserror enums
  status: pending
- id: p6-lock-in
  content: Workspace too_many_lines lint, delete size allowlist, machete, update docs
  status: pending
---

# Codebase simplification: 500-line files, 50-line functions, cleaner crates

Bring all 1,376 Rust files (471k lines) under 500 lines and all functions under 50 lines, remove duplicated helpers and dead code, cut needless dependencies between crates, and simplify the largest types. No behavior changes; each step is checked by fmt, clippy, tests, the invariant gate, a file-size gate and clippy's `too_many_lines` at 50.

## Todos

- [x] Land or stash WIP; record clippy, test, file-size and fn-length baseline
- [x] Add check_file_size.sh with shrinking allowlist and wire into CI
- [x] Add clippy.toml with too-many-lines-threshold = 50; fix 3 gate test fns
- [x] Enable unwrap_used/expect_used lints; fix desktop expects (@implement)
- [x] Remove 1.blob, test.wav, index.html from the repo root
- [x] Remove redundant crate-wide #![allow] blocks and fix the findings (@rust-engineer)
- [x] Drop dead_code allows in aura-agent verify/planning/file_ops and delete unused code (@rust-engineer)
- [x] Switch CI clippy to -D warnings and update development.md
- [x] Consolidate now_ms, truncation, formatting, console block and strip_ansi helpers (@rust-engineer)
- [x] Point aura-app at aura-ui instead of aura-ui-terminal
- [x] Move check_bearer and files_api down; drop aura-app's aura-node dependency (@rust-engineer)
- [x] Fold plugin-api, plugin-connectors, plugin-mcp into aura-plugin (@rust-engineer)
- [x] Extract agent_registry and agent_definitions into aura-engine-registry (@rust-engineer)
- [x] Move the cloud hub into aura-engine-cloud; keep host_info in aura-app (@rust-engineer)
- [x] Extract aura-ui-terminal leaf modules into aura-ui-widgets (@rust-engineer)
- [x] Files and functions pass: core, config, store (@rust-engineer)
- [x] Files and functions pass: model, context, plugin (@rust-engineer)
- [x] Files and functions pass: exec and agent crates; DomainError (@rust-engineer)
- [ ] Files and functions pass: auth and engine crates (@rust-engineer)
- [x] Group App's 158 fields into ListState and sub-state structs
- [x] Files and functions pass: aura-ui and aura-ui-terminal
- [~] Files and functions pass: app, node, cli, desktop, root tests
- [ ] Replace remaining anyhow in library crates with thiserror enums
- [ ] Workspace too_many_lines lint, delete size allowlist, machete, update docs

<!-- end of generated header: edit below -->

## What the review found

The workspace has 50 crates in 13 layers, and the layering is in good shape: `tests/layer_boundary.rs` enforces it, docs are thorough, and production code has almost no `unwrap`/`expect`. The gate layer is the model to follow: 9 crates, about 28k lines, no file over 500 lines, no production function over 50 lines, and every crate already has `#![deny(clippy::too_many_lines)]`.

The problems are size and repetition:

- **262 of 1,376 files are over 500 lines** (265k lines, 56% of the code). 74 are over 1,000. They fall into three classes:
  - 78 drop under 500 just by moving their inline `#[cfg(test)] mod tests` into a sibling file.
  - 135 need real splits. They hold 114k lines of non-test code, mostly in `aura-ui-terminal` (38), `aura-app` (25), `aura-node` (14), `aura-desktop` (9), `aura-agent` (8) and `aura-model` (6).
  - 46 are test files, up to 7,066 lines (`app/tests.rs`, `renderer/tests.rs`).
- **About 930 of 19,318 functions are over 50 lines: 542 production and about 389 test functions.**
  - Of the production ones, 329 are 51 to 75 lines, 119 are 76 to 100, and 114 are over 100. These counts are physical lines, so they are an upper bound; clippy does not count blank and comment lines.
  - They cluster in `aura-ui-terminal` (112), `aura-app` (94), `aura-agent` (60), `aura-node` (43), `aura-exec-tools` (36) and `aura-model` (31).
  - The longest:
    - `app/flow/layout.rs:581` (433 lines)
    - `stream_pump/driver.rs:119` (383)
    - `gateway/middleware.rs::create_router` (363)
    - `aura-agent-trace/src/graph.rs::apply_action` (333)
    - `node.rs::run` (282)
    - `event_loop/mod.rs::handle_ui_event` (272)
- **Two crates are too big to be one compile unit.**
  - `aura-ui-terminal` is 93k lines. Its `App` struct has 158 fields.
  - `aura-app` is 82k lines, and its `event_loop` module alone is 51k lines.
- **Some dependencies only exist for re-exports or small helpers.**
  - `aura-app` depends on `aura-ui-terminal`, but only for `aura-ui` types that it re-exports, plus one `strip_ansi` call.
  - `aura-app` depends on the whole `aura-node` crate for just `check_bearer` and `files_api` (in `api_server.rs`).
- **The same helpers are copied in many places.**
  - `now_ms` is defined 19 times, even though `aura_core_types::time::now_ms` exists.
  - There are about 11 string-truncation helpers.
  - Three console modules (`aura-model/src/console.rs`, `aura-agent/src/console.rs`, `aura-node/src/inbound_console.rs`) each carry their own `wrap_row`, `detect_term_width`, `wrap_value_chunks`, `human_bytes` and `human_duration_ms`.
  - `aura-agent::console::anthropic_request_block` and `anthropic_response_block` are dead copies of the `aura-model` ones.
- **Lint and dead-code debt.**
  - CI runs clippy with `-W warnings`, while rules.md requires `-D warnings`.
  - Several `lib.rs` files have crate-wide `#![allow(...)]` blocks. Some re-allow the lints the workspace deliberately turned on (`manual_let_else`, `needless_pass_by_value`, `unnecessary_wraps`, the `cast_*` lints). Others repeat allows for pedantic/nursery lints, which the workspace already allows.
  - Three `aura-agent` modules (`verify`, `planning`, `file_ops`, about 6k lines) carry `#![allow(dead_code)]` across the whole module. There are also 26 item-level `dead_code`, 25 `unused_imports`, 28 `too_many_arguments` and 4 `too_many_lines` allows.
- **`anyhow` in library crates.**
  - The `DomainApi` seam trait in `aura-exec-traits/src/domain.rs` returns `anyhow::Result` from every method.
  - `aura-model`, `aura-exec-*`, `aura-agent`, `aura-engine` and `aura-engine-automaton` also use `anyhow`. rules.md reserves it for application boundaries.
- **Stray tracked files at the repo root:** `1.blob` (empty), `test.wav` (882 KB) and `index.html`. Nothing references them. There are also two untracked `C:\...debug.log` files.

## The two hard limits and how they are enforced

**Files: 500 lines.** A file counts every line, tests included. A new `scripts/check_file_size.sh` enforces it, starting from an allowlist that may only shrink.

**Functions: 50 lines.** This applies to production and test functions alike, and uses clippy's own measure: `clippy::too_many_lines`, which counts lines of code in the body and skips blank and comment-only lines.

- Set `too-many-lines-threshold = 50` in a new root `clippy.toml`. This also tightens the gate crates' existing `deny` from clippy's default of 100 down to 50.
- When a crate is finished, it adds `#![deny(clippy::too_many_lines)]` to its `lib.rs`, the pattern the gate crates already use, so it cannot regress.
- At the end, `too_many_lines = "warn"` goes into `[workspace.lints.clippy]`. Together with CI's `-D warnings`, that covers integration-test crates too, and the per-crate attributes can then be deleted.
- No `#[allow(clippy::too_many_lines)]` is permitted. The 4 that exist today are removed.

## How code gets split without losing functionality

Each step is a pure refactor: no behavior, output or wire-format changes. Refactor-only commits stay separate from any logic change, so they review as moves.

**Files**

- Public paths stay stable. When an item moves, the old module re-exports it with `pub use`, so downstream crates don't change in the same commit.
- Module layout follows the dominant convention in the repo (145 `mod.rs` against 25 `foo.rs` + `foo/` pairs): a split `foo.rs` becomes `foo/mod.rs` that holds declarations, re-exports and the main type, plus one file per concern.
- Large `impl` blocks are split by behavior across files in the same module. Rust allows several `impl X` blocks, so `impl CloudHub` (about 1,780 lines in `cloud/hub.rs`) becomes `hub/place.rs`, `hub/recall.rs`, `hub/turns.rs` and `hub/links.rs` with no API change.
- Inline test modules move to a sibling `tests.rs`, declared as `#[cfg(test)] mod tests;`. Test files over 500 lines become a `tests/` directory split by topic, with shared fixtures in `tests/support.rs`.

**Functions.** The same few patterns cover almost every case.

- Sequential bodies become a short top-level function that calls named private steps (`prepare_turn_context` becomes resolve, build, attach, emit). The top-level function then reads as the outline of what happens.
- Big `match` statements over `UiEvent`, `UiCommand`, slash commands and keys dispatch each domain to its own handler module. Each arm becomes a one-line call. This covers `handle_ui_event`, `process_command`, `slash.rs`, `handle_normal_key` and `vim::handle_key`.
- Router and builder functions (`create_router`, `node.rs::run`, `build_kernel_with_config`) build one sub-router or component per area and compose them with `merge`/`nest`.
- Event loops with long `tokio::select!` blocks (`stream_pump/driver.rs`, `run_link_socket`) move their state into a struct with one method per branch, so a branch body never sits inside the loop. This also avoids passing many borrowed locals across `.await`.
- Renderers (`render_chat_lines`, `build_swarm_lines`, `flow/layout.rs`) are split into per-section functions that return their lines or rects, and the caller concatenates them.
- When extraction produces long parameter lists, the parameters go into a small context struct or become methods on the existing state struct. `#[allow(clippy::too_many_arguments)]` is not used, and the 28 existing ones are removed the same way as their crates are done.
- Tests: long setup moves into builders and fixtures in `tests/support.rs`. Repeated cases become table-driven loops over `[(input, expected)]` arrays. A test that checks several behaviors is split into one test per behavior, which also gives clearer failure names.

**Visibility.** Default visibility is `pub(crate)`. Each crate's pass enables `unreachable_pub` for that crate and narrows whatever it flags. New helper functions are private unless a sibling module needs them.

**Invariant gate.** `scripts/check_invariants.sh` names 21 files in path allowlists, for example `cloud/hub.rs`, `agents/host.rs` and `scheduler.rs`. When one of those files splits, the allowlist entry narrows to the submodule that holds the allowed call, and `docs/invariants.md` is updated in the same commit.

**New crates** get a `//! Layer:` tag and an entry in `[workspace].members`. They also inherit the `profile.dev` opt-level of the crate they came from: `aura-ui-terminal`/`aura-ui` use 1, `aura-store-db`/`aura-core-types` use 2.

Every step is checked with:

```sh
cargo fmt --all -- --check
cargo clippy -p <crate> --all-targets --all-features -- -D warnings
cargo test -p <crate> --all-features
cargo test -p aura --test layer_boundary
bash scripts/check_invariants.sh
bash scripts/check_file_size.sh
```

## Phase 0: Clean start and guardrails

- Land or stash the current uncommitted work, then record a baseline:
  - the clippy warning count
  - test results
  - the file-size report
  - the per-crate `too_many_lines` count at threshold 50
- Add `scripts/check_file_size.sh` and `scripts/file_size_allowlist.txt` (today's 262 files, may only shrink), and run them in `invariants.yml`.
- Add the root `clippy.toml`:

```toml
too-many-lines-threshold = 50
allow-unwrap-in-tests = true
allow-expect-in-tests = true
```

- Fix the 3 gate-crate test functions over 50 lines, so the gate crates' existing `deny` passes at the new threshold.
- Enable `clippy::unwrap_used` and `clippy::expect_used` in the workspace lints, and fix the roughly 22 `expect` calls in `aura-desktop` and the handful elsewhere.
- Remove `1.blob`, `test.wav` and `index.html` from the root after a last check that nothing uses them.

## Phase 1: Lint and dead-code hygiene

This phase deletes code before anything is split, so nothing dead gets refactored.

- Delete crate-wide `#![allow]` entries for pedantic and nursery lints. The workspace already allows those groups, so these entries are noise. Examples: `module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`, `doc_markdown`, `option_if_let_else`, `missing_const_for_fn`.
- Remove the crate-wide re-allows of the curated lints in `aura-app`, `aura-cli`, `aura-node`, `aura-engine` and `aura-agent`, and fix the findings instead. The generated-proto allow in `aura-engine-cloud` stays.
- Drop the module-wide `#![allow(dead_code)]` in `aura-agent/src/verify/mod.rs`, `planning.rs` and `file_ops/mod.rs`, then delete what the compiler reports unused. Do the same for the item-level `dead_code`, `unused_imports` and `deprecated` allows, keeping only those with a documented reason (for example the platform-gated clipboard code).
- Delete the unused `anthropic_*_block` functions in `aura-agent/src/console.rs`.
- Once clippy is clean, switch CI to `-D warnings` and update `docs/development.md`.

## Phase 2: One copy of each shared helper

Each helper goes into the lowest crate that every caller can already depend on, so no new upward edges appear.

| Helper | Copies | Single home |
|---|---|---|
| `now_ms` | 19 | `aura_core_types::time`, re-exported by `aura-ui` for the UI and desktop crates |
| Char-based truncation and short ids | about 8 | new `aura_core_types::text` |
| Width-based truncation and duration/byte formatting for the UI | about 6 | `aura_ui::format` |
| Console block layout (`wrap_row`, `detect_term_width`, `wrap_value_chunks`, `human_bytes`, `human_duration_ms`, `collapse_for_row`, `destination_tag`) | 3 | `aura_model::console::block`. Both `aura-agent` and `aura-node` already depend on `aura-model`. |
| `strip_ansi` | 1, but in the wrong crate | `aura_ui::ansi` |

## Phase 3: Cut needless dependencies between crates

- Point `aura-app` at `aura-ui` instead of `aura-ui-terminal`, and rewrite its roughly 125 `aura_ui_terminal::{events, UiCommand, repo, host, ...}` imports. The session host then builds in parallel with the 93k-line renderer.
- Move `check_bearer` into `aura-auth-kit`, and `files_api` (the workspace walker and reader with `MAX_WALK_DEPTH` and `MAX_READ_BYTES`) into `aura-exec-tools`. `aura-node` re-exports both, and `aura-app` drops its dependency on `aura-node`.
- Fold `aura-plugin-api`, `aura-plugin-connectors` and `aura-plugin-mcp` (about 1.2k lines, each used only by `aura-plugin`) into `aura-plugin` as modules. `aura-plugin-hooks` stays a crate because the agent and kernel use it. The rule behind this: a part is a crate when it has more than one consumer or is a compile boundary worth having, otherwise it is a submodule.
- Extract `aura-app/src/agent_registry` and `agent_definitions` (about 6k lines, used from 21 files, depending only on core, store, skills, exec and model) into a new engine-layer crate, `aura-engine-registry`.
- Move the cloud hub (`aura-app/src/cloud` minus `host_info.rs`, about 4k lines) into `aura-engine-cloud`, which owns the fleet already. `host_info.rs` builds UI views, so it stays in `aura-app`. The invariant allowlist entries for `cloud/hub.rs` and `cloud/store.rs` move with it.
- Extract the leaf modules of `aura-ui-terminal` into a new `aura-ui-widgets` crate (opt-level 1): `themes`, `graphics`, `animation`, `ansi`, `clipboard` and `components`, including syntax highlighting. Only 3 `components` files reach back into `app`/`renderer`, and those references are inverted to parameters first.

## Phase 4: One pass per crate for files and functions

Each crate is touched once. Splitting files and functions together keeps each file from being rewritten twice, and a function split often decides where a file should be cut. Crates are done from the bottom of the layer stack up, so each later crate builds on settled paths.

Within each crate the order is:

1. Move inline tests out.
2. Break up long production functions.
3. Split production files by concern.
4. Break up long test functions and split test files.
5. Add `#![deny(clippy::too_many_lines)]` and shrink the file allowlist.

| Order | Crates | Files (non-test) | Fns over 50: prod / test | Main targets |
|---|---|---|---|---|
| 1 | core, config | 4 | 3 / 4 | `types/compare.rs`, `types/trace.rs`, `ids.rs` |
| 2 | store | 4 | 6 / 13 | `channels.rs` becomes types, keys, and store split into channels/members/posts/links. Also `processes.rs`, `rocks_store/mod.rs` |
| 3 | model | 6 | 31 / 37 | `anthropic/provider.rs` (4,473 lines) becomes `waf.rs`, `diagnostics.rs`, `body_cap/`, `errors.rs`, `retry.rs` and `provider.rs`. `complete_streaming`, `classify_retry_action`, `into_response` |
| 4 | context, plugin | 6 | 35 / 10 | `messages.rs` becomes config/tiers, redaction, summary, dedup. `retrieve_with_query` |
| 5 | exec | 6 | 40 / 23 | `fs_tools/cmd/mod.rs`, `plan_tools.rs`, `web/html.rs`, `send_to_agent.rs` |
| 6 | agent | 12 | 91 / 73 | `stream_pump/driver.rs`, `tool_pipeline.rs`, `agent_loop/turn.rs`, `sampling.rs`, `graph.rs::apply_action`, `derivation.rs::derive` |
| 7 | auth, engine | 8 | 47 / 18 | `agents/host.rs`, `scheduler.rs`, `fleet.rs::provision_reported`, `snapshots.rs`, `boxd.rs`, `dispatch.rs` |
| 8 | ui | 1 | 3 / 0 | `events.rs` becomes `event.rs`, `command.rs` and `views/` per domain (channels, tasks, policies, processes, boot, usage, agents). The enums stay flat. |
| 9 | ui-terminal | 38 | 112 / 69 | Group `App` state first (Phase 5), then `flow/layout.rs`, `commands.rs::process_command`, `slash.rs`, `keys.rs`, `vim.rs`, `mouse.rs`, `chat_panel.rs`, and the 7k-line test files |
| 10 | app | 25 | 94 / 71 | `handle_ui_event`, `log_view.rs::step_view`, `harness.rs`, `session.rs::start`, `subagents.rs`, `flow_builder.rs`, `record_loader.rs`, `model_selection.rs` |
| 11 | node, cli, desktop, root `tests/` | 25 | 80 / 68 | `create_router`, `node.rs::run`, `prepare_turn_context`, `run_generation_proxy`, `session/helpers.rs`, `router.rs`, `shapes.rs::symbol`, `e2e_full.rs` |

Rows 1 to 5 are small and can go quickly. Rows 6, 9, 10 and 11 hold about three quarters of the function work and can be split into several PRs by module.

## Phase 5: Simplify the largest types and the error model

- Split `App`'s 158 fields into grouped sub-states. This lands at the start of the `aura-ui-terminal` pass, because methods on small sub-states are what make that crate's 112 long functions easy to break up.
  - A generic `ListState<T> { items, selected, scroll }` replaces the repeated triples (`records`/`selected_record`/`records_scroll`, and likewise for agents, definitions, archived and processes).
  - `InputState` gets `input`, `cursor_pos`, histories, `slash_cursor`, mentions and images.
  - `SelectionState` gets `selection`, `selecting`, the scrollbar grab and the copy flags.
  - `Notifications` gets the three notification fields.
  - `PanelLayout` gets the visibility and expanded flags.
- Replace `anyhow` in library crates with `thiserror` enums that keep context (ids, paths, status codes):
  - Start with `DomainError` for the `DomainApi` trait in `aura-exec-traits`, with mapping in `aura-exec-domain-http` (63 uses).
  - Then do `aura-exec-tools`, `aura-agent`, `aura-engine` and `aura-engine-automaton`, each during its Phase 4 pass.
  - `anyhow` stays only in `aura-cli`, `aura-app`, `aura-node` and `aura-desktop` top-level handlers.

## Phase 6: Lock it in

- Delete `scripts/file_size_allowlist.txt` once it is empty, so the gate becomes a hard 500-line limit.
- Add `too_many_lines = "warn"` to `[workspace.lints.clippy]` (CI's `-D warnings` makes it fatal) and delete the per-crate `deny` attributes.
- Run `cargo machete` and drop the dependencies that the merges and moves left unused.
- Update the crate tables and diagrams in `docs/architecture.md`, the gates and the two limits in `docs/development.md` and `.cursor/rules.md`, and the allowlist paths in `docs/invariants.md`.

## What could go wrong

- **Merge conflicts.** There is uncommitted work and 8 git worktrees under `.worktrees/` with branches in flight. About 930 function rewrites and about 260 file moves will conflict with them. Landing the open work first, keeping one crate (or module) per PR and merging quickly keeps the window small.
- **Hot-path performance.** Function extraction is nearly always inlined, but the per-frame TUI renderer, SSE parsing and the record decoder are performance-sensitive. Rows 3, 6, 9 and 11 compare these before and after:

```sh
cargo test --release -p aura-ui-terminal -- --ignored --nocapture timings
cargo test --release -p aura-desktop -- --ignored --nocapture timings
cargo test --release -p aura-agent --test many_agents_load -- --ignored --nocapture
```

  Any regression gets an `#[inline]` or a different cut.
- **Async borrows.** Splitting async functions can run into borrows held across `.await`. The state-struct-with-methods pattern above avoids most of these, and nothing is changed to `Arc`/clone just to satisfy the split unless the clone is trivially cheap.
- **Invariant gate.** Path allowlists in the invariant script break silently if a split forgets them. Running the script on every step catches this.
- **Build speed.** New crates without the matching `profile.dev` opt-level would make the dev TUI slower. The ground rules require copying it.
- **Hidden output changes.** Snapshot tests (`insta`, the CLI help goldens, the desktop PNG snapshots) guard against output changes. Any snapshot diff means a step was not a pure refactor and gets reverted.
- **Which lines count.** Clippy's measure skips blank and comment lines, so a function can be over 50 physical lines and still pass. If you want a strict physical-line limit, the file-size script can measure that too.
