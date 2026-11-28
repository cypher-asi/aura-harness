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

### Every task gets its own branch

##### Added

- `git_start_task` starts a task on a fresh branch,
  `aura/<task>-<your code>`, off the latest main. A name already taken
  gets a number, so no branch is reused, and it refuses while
  uncommitted or unpushed work would be left behind.

##### Changed

- A cloud machine may push any branch of its own agent, not only the one
  it was placed on, so it no longer piles every task onto one branch.
  Another agent's branch and main are still refused.
- Shipping more work on a branch that was merged already moves the new
  commits to a fresh branch first.

### A broken main is found and reported, not blamed on the next push

##### Fixed

- When a test failed on a push's merge, the queue failed the push, even
  when main itself failed that test (as when the CLI's help snapshot went
  stale on main and every push after it bounced). Now the failing tests
  run once more, then on main alone. A test that passes the second time
  merges as flaky and is named. A test that fails on main too parks the
  push without blame until main moves, while the fix still lands, and
  the channel's coordinator and the agents whose merges may have broken
  it are told which tests fail, their output, and the commits since main
  last passed.

##### Changed

- The QUEUE tab shows a broken main above every merge, and a failed
  merge's failing tests and their diff first, then its PR.

### The merge queue lands exactly the commit it tested

##### Fixed

- A PR now lands as the merge commit the queue built and tested, pushed
  to main without force, and GitHub marks the PR merged. Before, the queue
  asked GitHub to merge the PR, which made a new merge commit on whatever
  main was by then, one no check had run on.
- A PR closed by hand while the queue tests it fails its entry and keeps
  its branch; before, the branch was deleted.
- A failing test counts as flaky only when the whole test command passes
  when run again, and the PR gets a comment naming the flaky tests.

### The merge queue's GitHub calls are declared, and fewer ways around it

##### Changed

- `run_command` also refuses GitHub API calls that merge or move a
  branch: `gh api` writes to `/merges` or `/git/refs`, GraphQL
  `mergePullRequest` and `enablePullRequestAutoMerge`, and the same
  through `curl` to `api.github.com`. Branch protection stays the real
  guard.
- GitHub refusing the queue's credentials (401/403) fails the entry with
  a message on how to sign the hub in, instead of holding the queue.
- A PR whose head is already on main is closed with a comment and its
  branch deleted.
- A second push that fails the same tests on a main already found broken
  parks at once, without running them twice more.
- Starting a task refuses when it can't tell which commits origin lacks,
  and a follow-up branch name the machine may not push is refused before
  anything is checked out.
- Everything the queue writes through `gh` is declared, and the
  invariant check catches `gh` run through a variable.
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
