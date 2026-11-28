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
- `cargo test` runs doctests only in the two crates that have them, and
  links no test binary for the binaries and the libraries without unit
  tests.
- Each crate's integration tests build as one binary, `tests/it/main.rs`,
  instead of one per file, so `cargo test --workspace` links 18
  integration-test binaries where it linked 63. Run one file's tests with
  `cargo test -p <crate> --test it <file>::`.
- HTTP requests go through reqwest 0.12 on rustls, the TLS stack the rest
  of aura already used, instead of reqwest 0.11 on OpenSSL. Builds drop a
  second copy of hyper, h2 and http along with OpenSSL, and the Docker
  images no longer install `libssl-dev`. Certificates are still checked
  against the system's trust store, and requests still use HTTP/1.1.
  Request errors name their cause (a refused connection, a DNS, TLS or
  JSON error) as before.
- `scripts/build-cloud-binary.sh` builds with a new `cloud-dev` profile:
  release optimizations without cross-crate LTO, compiled incrementally,
  so the rebuild after a node change takes about 30 s instead of 3
  minutes. Pass `--release` for a binary you ship.
