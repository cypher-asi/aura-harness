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

### The merge queue lands every push through its pull request

##### Changed

- A push of a repo on GitHub now lands through its PR. The queue finds
  or opens the branch's PR, merges the latest main into it, builds and
  tests that, puts the tested merge on the PR's branch, and merges the
  PR with a normal merge commit, only at the head it tested
  (`gh pr merge --merge --match-head-commit`). Before, PRs were merged
  beside the queue, so a stale head or an untested merge could reach
  main.
- A conflict, a failing check or a refused merge leaves the PR open with
  a comment saying why (the conflicting files, or the output) and an
  `aura/merge-queue` failure status; the agent is told the PR, the files
  and to fix it on the same branch. A head that moved after the test is
  tested again before it merges.
- A merged branch, or one whose PR was closed by hand, is deleted on
  GitHub, unless something was pushed to it since.
- When GitHub can't be reached, the queue waits and retries instead of
  failing the entry, and the agent hears that it waits through no fault
  of its own.
- `git_ship` pulls its own branch before pushing, so a fix after the
  queue put a merge on it is still a fast-forward.
- `run_command` refuses `gh api` calls that merge a PR, as it already
  refused `gh pr merge`.
