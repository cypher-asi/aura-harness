# Development

## Everyday commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -W warnings
cargo test --workspace --all-features

# Invariant grep gate
bash scripts/check_invariants.sh

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

- `cargo fmt --check`, `cargo clippy` (warnings reported, not denied), and `cargo test --workspace --all-features`.
- `cargo check` on the MSRV (1.85), except `aura-desktop`, which needs 1.88 for wgpu.
- `cargo deny check` (hard fail) and `cargo audit` (soft fail).
- `cargo machete` for unused dependencies.
- [`scripts/check_invariants.sh`](../scripts/check_invariants.sh), the invariant grep gate.

Each invariant's enforcement (grep band or Rust test) is listed in [invariants.md](invariants.md#enforcement-map).

## Where to change things

- Adding a crate: put it in `crates/<layer>/`, name it `aura-<layer>-<part>`, add a `//! Layer: <layer>` tag to its `lib.rs`, and add it to `[workspace].members` in [`Cargo.toml`](../Cargo.toml) with the rest of its layer. Path dependencies within a layer are `../aura-…`; across layers they are `../../<layer>/aura-…`. Only surface crates, which are named after what you run, need an entry in `KNOWN_CRATES` in [`tests/layer_boundary.rs`](../tests/layer_boundary.rs). See [architecture.md](architecture.md#naming).
- Adding a tool: classify it in the mode gate table ([`mode_gate.rs`](../crates/agent/aura-agent-kernel/src/policy/check/mode_gate.rs)); [`mode_gate_catalog.rs`](../crates/engine/aura-engine/tests/mode_gate_catalog.rs) fails until you do.
- Changing the wire types: update [`aura-core-protocol`](../crates/core/aura-core-protocol), regenerate the TypeScript bindings with `cargo test -p aura-core-protocol --features typescript export_typescript_bindings`, and land the matching change in `aura-os`.
- Release notes go in [`CHANGELOG.md`](../CHANGELOG.md).
