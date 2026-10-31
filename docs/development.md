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
cargo check -p aura-core-types -p aura-core-modes -p aura-protocol -p aura-config -p aura-model-reasoner
```

## CI gates

[`.github/workflows/`](../.github/workflows) runs, on every PR:

- `cargo fmt --check`, `cargo clippy` (warnings reported, not denied), and `cargo test --workspace --all-features`.
- `cargo check` on the MSRV (1.80).
- `cargo deny check` (hard fail) and `cargo audit` (soft fail).
- `cargo machete` for unused dependencies.
- [`scripts/check_invariants.sh`](../scripts/check_invariants.sh), the invariant grep gate.

Each invariant's enforcement (grep band or Rust test) is listed in [invariants.md](invariants.md#enforcement-map).

## Where to change things

- Adding a crate: name it `aura-<layer>-<name>`, add a `//! Layer: <layer>` tag to its `lib.rs`, register it in [`tests/layer_boundary.rs`](../tests/layer_boundary.rs) if the name doesn't follow the convention, and add it to `[workspace].members` in [`Cargo.toml`](../Cargo.toml). See [architecture.md](architecture.md).
- Adding a tool: classify it in the mode gate table ([`mode_gate.rs`](../crates/aura-agent-kernel/src/policy/check/mode_gate.rs)); [`mode_gate_catalog.rs`](../crates/aura-engine/tests/mode_gate_catalog.rs) fails until you do.
- Changing the wire types: update [`aura-protocol`](../crates/aura-protocol), regenerate the TypeScript bindings with `cargo test -p aura-protocol --features typescript export_typescript_bindings`, and land the matching change in `aura-os`.
- Release notes go in [`CHANGELOG.md`](../CHANGELOG.md).
