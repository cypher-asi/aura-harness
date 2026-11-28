# Development

## Everyday commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# After adding, removing or changing a dependency (CI fails until you do)
cargo hakari generate && cargo hakari manage-deps

# Invariant grep gate
bash scripts/check_invariants.sh

# No .rs file over 500 lines
bash scripts/check_file_size.sh

# Layer rule (no crate may depend on a higher layer)
cargo test -p aura --test layer_boundary

# Crates that build without RocksDB / LLVM
cargo check -p aura-core-types -p aura-core-modes -p aura-core-protocol -p aura-config -p aura-model

# Desktop app screens, rendered offscreen and saved as PNGs
AURA_DESKTOP_SNAPSHOTS=target/snapshots cargo test -p aura-desktop snapshot

# Desktop timings: boot, frames while streaming and typing, a swarm's second,
# a 2,000-message conversation and a long reply streaming after it
cargo test --release -p aura-desktop -- --ignored --nocapture timings

# The same conversation and reply in the terminal UI alone
cargo test --release -p aura-ui-terminal -- --ignored --nocapture timings

# Many agents at once: turns through the kernel and record store, and the
# terminal UI applying and drawing their streams
cargo test --release -p aura-agent --test many_agents_load -- --ignored --nocapture
cargo test --release -p aura-ui-terminal --test ui_load -- --ignored --nocapture
```

`cargo clippy` runs every check `cargo check` does, so run one, not both. CI also builds with `--all-features`, which adds only the TypeScript bindings export; you need it locally only when you change the wire types (see [Where to change things](#where-to-change-things)).

### Keeping builds fast

Cargo keeps one build of a crate per set of features, flags and dependencies, and rebuilds a crate and everything above it when any of those changes. So:

- Every crate depends on [`workspace-hack`](../workspace-hack/Cargo.toml), which [cargo-hakari](https://docs.rs/cargo-hakari) fills with every feature any crate turns on in a dependency. `cargo build`, `cargo test`, `cargo clippy` and `-p` runs then build each dependency one way and reuse each other's work. Install it with `cargo install cargo-hakari --locked`, and after a dependency change run `cargo hakari generate && cargo hakari manage-deps`.
- Don't put test helpers behind a cargo feature. A feature only tests turn on is a second build of the crate and of every crate above it, and the build flips between them each time you go from `cargo test` to `cargo run`. Compile the helpers in and mark them `#[doc(hidden)]`, as `aura_store_db::FaultAt` is.
- Don't pass `--all-features` or set `RUSTFLAGS` for one command; either rebuilds the workspace, and again when you go back. Flags you always want belong in `~/.cargo/config.toml`.
### Profiling under load

The two load tests take their sizes from the environment:

- `AURA_LOAD_AGENTS` is the list of agent counts to run, for example `1,8,32,64`.
- `AURA_LOAD_STEPS` and `AURA_LOAD_RESULT_BYTES` set the tool steps in each agent's turn and the size of each result.
- `AURA_LOAD_FRAMES` sets the frames the UI is streamed for.

`many_agents_load` prints wall time, turn p50 and p99, iterations per second, CPU time and resident memory for each count. `ui_load` prints the time to apply a frame's commands, the time to draw it, and how many of 100 idle ticks still redrew. Compare a change against a run of the same command before it, on the same machine.

To see where the time goes, build the test once and run its binary under a sampling profiler:

```sh
cargo test --release -p aura-agent --test many_agents_load --no-run
perf record -g --call-graph dwarf -- target/release/deps/many_agents_load-<hash> --ignored --nocapture
perf report
# or, with cargo-flamegraph installed:
cargo flamegraph --test many_agents_load -p aura-agent -- --ignored --nocapture
```

`RUST_LOG=aura=debug` shows the kernel's and the loop's spans in the same run.

The desktop snapshot tests run the terminal UI in the window's shell, without a window, and draw whole screens (conversation, a working agent, the slash menu, dialogs) with the CPU rasterizer, so they run without a display or GPU. With `AURA_DESKTOP_SNAPSHOTS` set, each screen is saved as a PNG next to a `.txt` dump of its cells; without it they only check that something was drawn. With `AURA_DESKTOP_GPU=1`, on a machine with a graphics adapter, each screen is also drawn by the GPU and compared with the CPU's picture.

## CI gates

[`.github/workflows/`](../.github/workflows) runs, on every PR:

- `cargo fmt --check`, `cargo clippy` with warnings denied, and `cargo test --workspace --all-features`.
- `cargo check` on the MSRV (1.85), except `aura-desktop`, which needs 1.88 for wgpu.
- `cargo deny check` (hard fail) and `cargo audit` (soft fail).
- `cargo machete --skip workspace-hack` (cargo-machete 0.10 or newer) for unused dependencies.
- `cargo hakari generate --diff` and `cargo hakari manage-deps --dry-run`, which fail while `workspace-hack` is out of date.
- [`scripts/check_invariants.sh`](../scripts/check_invariants.sh), the invariant grep gate.
- [`scripts/check_file_size.sh`](../scripts/check_file_size.sh), the 500-line file limit.

Each invariant's enforcement (grep band or Rust test) is listed in [invariants.md](invariants.md#enforcement-map).

## Size limits

- No `.rs` file is over 500 lines, tests included; only the benchmark files under `evals/` are exempt. Split a long file into a module directory: `foo/mod.rs` plus one file per concern, with its tests in `tests.rs` or `tests/`, re-exporting what callers use so their paths stay the same.
- No function is over 50 lines. The workspace lints set `clippy::too_many_lines` for every crate and target, with the threshold in [`clippy.toml`](../clippy.toml), and CI denies the warning. Split a long function into named steps, and pass a struct rather than allowing `clippy::too_many_arguments`.

## Where to change things

- Adding a crate: put it in `crates/<layer>/`, name it `aura-<layer>-<part>`, add a `//! Layer: <layer>` tag to its `lib.rs`, and add it to `[workspace].members` in [`Cargo.toml`](../Cargo.toml) with the rest of its layer, then run `cargo hakari manage-deps` to give it its `workspace-hack` line. Path dependencies within a layer are `../aura-…`; across layers they are `../../<layer>/aura-…`. Only surface crates, which are named after what you run, need an entry in `KNOWN_CRATES` in [`tests/layer_boundary.rs`](../tests/layer_boundary.rs). See [architecture.md](architecture.md#naming).
- Adding a tool: classify it in the mode gate table ([`mode_gate.rs`](../crates/agent/aura-agent-kernel/src/policy/check/mode_gate.rs)); [`mode_gate_catalog.rs`](../crates/engine/aura-engine/tests/mode_gate_catalog.rs) fails until you do.
- Changing the wire types: update [`aura-core-protocol`](../crates/core/aura-core-protocol), regenerate the TypeScript bindings with `cargo test -p aura-core-protocol --features typescript export_typescript_bindings`, and land the matching change in `aura-os`.
- Release notes: add one file per change in [`changelog.d/`](../changelog.d/README.md), named after your branch's last segment (`changelog.d/fix-scroll-rus12.md`), with the entry as it would read under `## [Unreleased]`. Don't edit [`CHANGELOG.md`](../CHANGELOG.md) directly: every parallel branch's entry used to collide at its top, and `git_ship` now refuses such a push. Keep each entry in one place: `aura changelog --check` fails when a fragment's entry is also under `[Unreleased]` in `CHANGELOG.md` (a direct merge or a hand-resolved conflict can put one back), and so does `tests/changelog_fragments.rs`, which the merge queue's tests run. At release time, `aura changelog` previews the entries and `aura changelog --write` puts them into `CHANGELOG.md` and deletes the fragments; both refuse a duplicate.
