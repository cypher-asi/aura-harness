### The invariant check passes on main again

##### Fixed

- `scripts/check_invariants.sh` reported 9 violations on main, none of
  them real breaks:
  - Files that moved: `delta.rs` became `delta/`, so the check now names
    the folder.
  - Call sites never declared: the app's titles and post summaries
    (recorded through a `KernelModelGateway`), the offline gate replay,
    and `aura changelog`'s read-only git. They are now declared in
    `docs/invariants.md`.
  - A scratch example: `aura-app/examples/` may use `Arc<dyn Store>`,
    like tests.
  - A wrong match: the `changes.rs` check read flags such as `--binary`
    as git subcommands.
