### Builds reuse each other's work

##### Changed

- `cargo build`, `cargo run`, `cargo test`, `cargo clippy` and runs of one
  crate (`cargo test -p aura-store-db`) build every dependency with the
  same features, so going from one to another no longer recompiles tokio,
  RocksDB and the crates above them. A new `workspace-hack` crate, kept by
  `cargo hakari`, lists those features; after a dependency change, run
  `cargo hakari generate && cargo hakari manage-deps` (CI checks it).
  tokio's paused clock (`test-util`) is on in every build, and the test
  helpers that sat behind `test-support` features are compiled in, hidden
  from the docs.
- The everyday commands in `docs/development.md` and `.cursor/rules.md`
  drop `--all-features`, which rebuilt the whole workspace only to add the
  TypeScript bindings export.
- Each checkout builds into its own `target/` instead of a folder shared
  by every checkout, so worktrees building at once no longer wait on each
  other or pick up each other's builds. Run `scripts/setup-sccache.sh`
  once per machine to share compiled dependencies between checkouts.
