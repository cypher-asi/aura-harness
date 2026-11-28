### The workspace tests pass on cloud machines

##### Fixed

- `cargo test --workspace` failed two targets on every cloud machine
  (`e2e_full` and `e2e_live`, 503 where 400 was expected) and passed on
  the hub. The test server read the machine's own `AURA_CLOUD_NODE` and
  `AURA_WORKSPACE` and became a cloud node. It now never is one.
- A failed insta run's `*.snap.new` and `*.pending-snap` files are
  ignored, so a ship can't commit them.

##### Added

- `docs/software-factory-review.md`: where the plan, build and ship flow
  and the merge queue broke, conflicted or slowed down, ranked by cost,
  each with its evidence, root cause and fix.
