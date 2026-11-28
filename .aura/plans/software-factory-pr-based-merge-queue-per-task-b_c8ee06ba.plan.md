---
name: 'Software factory: PR-based merge queue, per-task branches, main-is-broken detection'
overview: Diagnose where the plan/build/ship flow and the merge queue lose time, then rebuild landing around GitHub PRs. The queue tests each PR and merges it pinned to the tested head. Every task gets its own branch, a broken main is detected instead of blamed on an entry, and stale CLI snapshots can't ship. The work ships as five small PRs.
todos:
- id: coordinate
  content: Message RUS198, RUS206, RUS204, RUS197 and RUS205 about the plan and the files I will touch
  status: pending
- id: pr1-env
  content: 'PR 1: make the e2e test server ignore machine env vars; ignore *.snap.new'
  status: pending
- id: pr1-review
  content: 'PR 1: write docs/software-factory-review.md and the changelog fragment; ship and wait for the merge'
  status: pending
- id: pr2-forge
  content: 'PR 2: add the forge module (Forge trait, GhForge with timeouts) and the landing config'
  status: pending
- id: pr2-land
  content: 'PR 2: land through the PR: tested-head push, pinned merge, comments, statuses, branch cleanup, moved-head retry'
  status: pending
- id: pr2-app
  content: 'PR 2: wire GhForge into the queue service, failure prompt with PR and files, git_ship pulls its own branch, push guard'
  status: pending
- id: pr2-tests
  content: 'PR 2: FakeForge tests (auto-merge, conflict, failing test, moved head, cleanup); docs; ship'
  status: pending
- id: pr3-rule
  content: 'PR 3: push rule allows only the agent''s own aura/*-<code> branches'
  status: pending
- id: pr3-tool
  content: 'PR 3: git_start_task tool, follow-up rebranch in git_ship, PR title and body in the marker'
  status: pending
- id: pr3-prompts
  content: 'PR 3: update the factory prompts and the cloud_git note with RUS197/198; tests; ship'
  status: pending
- id: pr4-broken
  content: 'PR 4: parse failing tests, flaky retry once, main-broken hold and suspects, hub reports'
  status: pending
- id: pr4-tab
  content: 'PR 4: Queue tab shows the held banner and the failing tests and diff first (with RUS206); tests; ship'
  status: pending
- id: pr5-snapshots
  content: 'PR 5: path-triggered ship checks for CLI snapshots, INSTA_UPDATE=no in checks; tests; ship'
  status: pending
- id: report
  content: 'Report to COO2: findings, PR links and merge commits, and what agents and the user must change'
  status: pending
---

# Software factory: PR-based merge queue, per-task branches, main-is-broken detection

Diagnose where the plan/build/ship flow and the merge queue lose time, then rebuild landing around GitHub PRs. The queue tests each PR and merges it pinned to the tested head. Every task gets its own branch, a broken main is detected instead of blamed on an entry, and stale CLI snapshots can't ship. The work ships as five small PRs.

## Todos

- [ ] Message RUS198, RUS206, RUS204, RUS197 and RUS205 about the plan and the files I will touch
- [ ] PR 1: make the e2e test server ignore machine env vars; ignore *.snap.new
- [ ] PR 1: write docs/software-factory-review.md and the changelog fragment; ship and wait for the merge
- [ ] PR 2: add the forge module (Forge trait, GhForge with timeouts) and the landing config
- [ ] PR 2: land through the PR: tested-head push, pinned merge, comments, statuses, branch cleanup, moved-head retry
- [ ] PR 2: wire GhForge into the queue service, failure prompt with PR and files, git_ship pulls its own branch, push guard
- [ ] PR 2: FakeForge tests (auto-merge, conflict, failing test, moved head, cleanup); docs; ship
- [ ] PR 3: push rule allows only the agent's own aura/*-<code> branches
- [ ] PR 3: git_start_task tool, follow-up rebranch in git_ship, PR title and body in the marker
- [ ] PR 3: update the factory prompts and the cloud_git note with RUS197/198; tests; ship
- [ ] PR 4: parse failing tests, flaky retry once, main-broken hold and suspects, hub reports
- [ ] PR 4: Queue tab shows the held banner and the failing tests and diff first (with RUS206); tests; ship
- [ ] PR 5: path-triggered ship checks for CLI snapshots, INSTA_UPDATE=no in checks; tests; ship
- [ ] Report to COO2: findings, PR links and merge commits, and what agents and the user must change

<!-- end of generated header: edit below -->

## What the evidence already shows

I read main's history (#52 to #117), `gh pr list` and the queue code. Main is at `90fa47f2`. Five findings shape the plan:

- **The queue has never landed a commit.** No commit on main says "through the merge queue". Every merge since #52 is a "Merge pull request #N" made by `cypher-asi-aura[bot]` (59 of them) or by the user (5), plus four direct `Merge branch 'main'` commits by n3o (`209d774f`, `ff244c14`, `58667928`, `f443075a`). Those four broke the build and needed #99, and `bbf6f9b7` added the duplicate entry that #100 removed. The queue ran only as a bystander, as in RUS203's failed entry at `8a1be374`.
- **Branches are reused for weeks.** `aura/software-factory-rus197` backs 9 PRs, `owed-reports-survive-restart-rus203` 8, `why-members-stall-rus202` 7 and `fewer-merge-conflicts-rus204` 6. The cause is the machine pin: `CloudNode::git_branch()` goes into `ToolConfig::push_branch`, and `resolve_branch` in `git_tool/push.rs` refuses every other name. That forced the `-s ours` supersede merges (`913e5eaf`, `d5db3396`, `02816ac1`) and keeps RUS205 on its encryption branch. Since then the branch has carried #93 and #113.
- **A stale head was merged.** #84 merged at `372e9521`, seconds before `b74c4bb9` arrived (#91), because `gh pr merge` didn't pin the head.
- **The same feature was built several times.** The HISTORY tab came from #68 (RUS197), #72/#77/#111 (RUS200) and #73/#78 (RUS201). The welcome banner came from #66 and #67, and the QUEUE tab work from #87 and #80.
- **Main is green, but not on cloud machines.** `cargo test --workspace` on this machine fails 2 targets: `-p aura --test e2e_full` and `--test e2e_live` answer 503 instead of 400. With `AURA_CLOUD_NODE` unset they pass (78 and 33 tests). `tests/common/server.rs::test_node_config` builds `NodeConfig::from_env()`, so the machine's `AURA_CLOUD_NODE=1` and `AURA_WORKSPACE` leak into the test server. Every agent sees a "red main" that isn't red. The `--help` snapshot already passes since #109.

## How the PRs ship while my machine is pinned

My machine can push only `aura/improve-the-software-factory-rus207` until the new push rule is deployed. So the PRs go out one at a time from that branch: push, then `gh pr create`. I start the next PR only after the previous one is merged (checked with `gh pr view N --json state,mergeCommit`). Each PR adds its lines to the one fragment, `changelog.d/improve-the-software-factory-rus207.md`. If a queue run fails, I fix it on the same branch.

Before touching their code, I message each owner by agent id:

- RUS198 (`fb29978512ec…`): start failures, for the review.
- RUS206 (`6d00d1a1b2ac…`): Queue tab.
- RUS204 (`76f2c9cf70cf…`): GitHub token and integration.
- RUS197 (`ff9c496fccc3…`): factory prompts, the `cloud_git` note and the hub's `bundle_branch` mirror.
- RUS205 (`f62a6c118e42…`): moving off its legacy branch.
- FEN6 and RUS200 to RUS203: only if I touch their files.

## PR 1: fix main for machines, and the review

- In `tests/common/server.rs` `test_node_config`, reset the machine-only fields after `from_env` (`cloud_node = false`, `workspace = None`, and whatever else the 503 traces back to). Verify with `AURA_CLOUD_NODE=1 cargo test -p aura --test e2e_full --test e2e_live`.
- Add `*.snap.new` and `*.pending-snap` to `.gitignore`. Today a failing insta run leaves these files behind, and `git_ship`'s `add -A` would commit them.
- Write `docs/software-factory-review.md`. It ranks the findings by cost (time lost, rework, broken main), and each has its evidence (PR numbers, shas), root cause and fix, ending with what was left out and why. It covers duplicate work, bypassing the queue, the conflicts, the slowdowns, unreliable checks, the `8a1be374` run, the env-leak red main, and cloud start failures with RUS198's fix and status.

## PR 2: the queue lands through GitHub PRs

All of this is in `crates/exec/aura-exec-merge-queue`. Every new file stays under 500 lines.

**`forge/` module.** It holds a `Forge` trait:

- `open_or_find_pr(branch, target, draft)`
- `comment(pr, body)`
- `set_status(sha, state, text)` with the context `aura/merge-queue`
- `merge(pr, head)`, which returns `Merged { sha }`, `HeadMoved { now }` or `Refused(reason)`
- `pr_state(pr)`

The `GhForge` implementation runs `gh` with `--repo owner/name` (parsed from the remote URL), a timeout on every call and an optional `GH_TOKEN`. It merges with `gh pr merge N --merge --match-head-commit <sha>`. On a failure it reads `gh pr view --json state,headRefOid,mergeCommit` to tell a moved head, a merge already done and a real refusal apart.

**`QueueConfig::landing`.** `land = "pull_request" | "push"` sets how an entry lands. The default is `pull_request` when the entry's remote is on github.com, and `push` for local repos, which keep today's path.

**`integrate/pull_request.rs`.** This is the landing sequence:

1. Fetch the target and the branch. A branch tip that moved since the push is the head that gets tested.
2. Find or open the PR and set its status to pending.
3. Merge in the queue's worktree, keeping the generated-file auto-resolution.
4. Run the checks.
5. When the queue made a merge commit, push it to the PR's branch as a fast-forward. A rejection means the head moved, so the entry is retried.
6. Confirm the target tip is unchanged with `ls-remote`, set the status to success, and merge pinned to that head.
7. Delete the branch on origin with `--force-with-lease=<branch>:<head>`.

On a conflict, a failed check or a refusal, nothing merges: the queue comments on the PR with the reason and the files or tests, sets the status to failure, and the entry fails. A moved head (409, or a push rejection) goes back through the `LAND_ATTEMPTS` loop. A PR closed without a merge is treated as abandoned: the entry fails and the branch is deleted. Crash recovery keeps `landed()`, and `pr_state == merged` also counts as landed.

**`MergeEntry` gains three fields**, all with serde defaults so old queue files still load: `pr: Option<PrRef { number, url }>`, `failure: Option<Failure { kind, files, tests, excerpt }>` and `tested_head`.

**The hub's queue service** (`aura-app/.../factory/service.rs`) builds a `GhForge`. The token comes from RUS204's GitHub integration when it has one, else the ambient `gh` login. `failure_prompt` names the PR link, the conflicting files or failing tests, and says "fix it on this same branch".

**`git_ship`** also fetches its own remote branch and merges it when the queue moved it. Without that, a fix push after a queue merge commit would be rejected.

**The push guard** (`fs_tools/cmd/push_guard.rs`) also refuses `gh api` calls on `pulls/<n>/merge`.

**`.aura/merge-queue.toml`** is committed with `land = "pull_request"`, `build = "cargo build --workspace --all-targets"` and `test = "cargo test --workspace"`.

**Tests** go in `tests/pull_requests.rs`. They use a `FakeForge` in `tests/support` that acts like GitHub over a local bare repo: it merges with `--no-ff` only when the head matches, records comments and statuses, and refuses stale heads. They cover:

- a ship that opens a PR and auto-merges;
- a code conflict that leaves the PR open with a comment and the files in the entry's failure;
- a failing test that blocks the merge;
- a moved head that is retested;
- branch cleanup after a merge, and after an abandoned PR.

In `service.rs`, a unit test checks that the agent's prompt holds the reason and the files.

## PR 3: one branch per task

All of this is in `crates/exec/aura-exec-tools/src/git_tool`.

**The push rule.** When `push_branch` is set, `resolve_branch` now allows that branch and any `aura/<slug>-<code>` with the same agent code (its last `-` segment). It still refuses the protected branches, the queue target and another agent's code, with a `BranchNotAllowed` that names the rule. `aura-node` needs no change.

**A new `git_start_task { task }` tool.** It fetches the target and creates `aura/<slug(task)>-<code>` at `origin/<target>`. A name that already exists locally or on origin gets a `-2`, `-3` suffix, so no name is reused. It refuses when there are uncommitted changes or unpushed commits and names them, so no work is lost. The agent code comes from the pin, else from the current `aura/...-<code>` branch. It is registered in:

- `GIT_TOOL_NAMES`
- the plan-mode gate (`policy/check/mode_gate.rs`)
- `aura-app/.../modes.rs`
- `aura-gate-lead/src/limits/tools.rs`
- `aura-agent/src/console.rs`

**Follow-up work on a merged branch.** When `git_ship` finds that the branch's last pushed tip is already in the target and new commits sit on top, it moves them to a fresh branch before pushing. It uses the slug of an optional `task` argument, else the old slug with a numeric suffix, and the result says so.

**PR text.** `git_ship` takes optional `pr_title` and `pr_body`, carried in the queue marker (serde-optional in `QueuedPush`), so the queue opens the PR with the agent's own text.

**Prompts** say to start every task with `git_start_task` and that the queue opens the PR. They change in `FACTORY_RULES` and `SHIP_PROMPT` (agreed with RUS197) and in the `cloud_git` note in `aura-node/.../chat/turn.rs` (agreed with RUS197 and RUS198).

**Tests:**

- two tasks from one agent land on separate branches;
- a machine pinned as rus207 is refused `aura/x-rus204`, `main` and the target;
- follow-up work after a merge gets a new branch;
- a dirty tree refuses `git_start_task`.

## PR 4: main-is-broken detection, flaky retry, failing tests first

**A new `failure.rs` module** parses libtest output: the `test x ... FAILED` lines and each `---- x stdout ----` block, which holds the assertion or insta diff. It is unit-tested on captured cargo output.

**When the test check fails,** the queue runs the failing tests once more on the same commit; for `cargo test` commands it appends `-- --exact <names>`, other commands run whole. If they pass the second time, the entry merges and the note names the flaky tests. If they fail again, the queue runs them on the plain target. If they fail there too, `RunOutcome::Held(MainBroken)`: the entry goes back to queued in its place, without counting an attempt.

**`QueueState::hold`** pauses `claim_next` until the target tip changes, and the processor polls `Integrator::target_tip`. The same hold, retried every 5 minutes, covers GitHub being unreachable. Neither case bounces an entry.

**The hub reports a broken main.** A `QueueEvent::Held` makes the hub send a notice to the coordinator of each channel on the repo and to the suspects. Suspects are the first-parent commits since `QueueState::last_green`, each mapped to the queue entry that merged it, or named "merged outside the queue" with sha, author and subject.

**The Queue tab.** `MergeEntryView` gains `failing_tests`, `failure_excerpt` and `pr`, and `MergeQueueView` gains `held`. `channel_queue.rs` draws the held banner on top, and for a failed entry the failing tests and the diff before its other lines. The content and order are mine; the styling goes through RUS206.

**Tests:**

- a broken main is held and reported, not blamed on the entry;
- a test that fails once then passes merges as flaky;
- a test that fails on the entry but passes on main fails the entry, with the test name;
- the view lists the tests first.

## PR 5: stale snapshots can't ship

- `.aura/merge-queue.toml` gains `[[ship_check]]` entries, each a list of path prefixes and a command. The first one covers the CLI's argument and help sources (`src/`, `crates/surface/aura-cli/src/`, `tests/cli_golden.rs` and `tests/snapshots/`) and runs `cargo test -p aura --test cli_golden`. The exact paths are confirmed when building.
- After the pull, `git_ship` runs every check whose paths match `git diff --name-only <target>...HEAD`, before it pushes. It fails with `ShipCheckFailed`, which carries the failing test and the end of its output.
- Both the ship checks and the queue's checks run with `INSTA_UPDATE=no` and `CI=true`, so a mismatch fails instead of writing new snapshot files.
- Tests cover a matching path running its check and blocking the push, and a non-matching change skipping it.

## Left out, and why

- **Running only the affected crates' tests.** Workspace-level tests such as `every_crate_carries_a_matching_layer_doc_tag` and `cli_golden` would be skipped, and that is how main went red before.
- **A shared build cache.** The queue's worktree already keeps `target/`.
- **A clippy baseline.** The user said to ignore CI, and the queue doesn't run clippy.
- **File claims at plan time.** They would block parallel work, and the overlap warnings already make agents talk.
- **GitHub's own merge queue.** It needs Actions, which are locked by billing.

## What could go wrong

- **The hub's `gh` isn't signed in.** The queue holds and says so instead of failing entries.
- **A PR is merged by hand.** Statuses alone don't stop that. The user has to turn on branch protection on `main` that requires the `aura/merge-queue` status; the review doc gives the exact `gh api` command.
- **The hub runs old code.** The new queue takes effect only once the user rebuilds and restarts the hub.
- **The hub's mirror misses per-task branches.** The mirror bundles only the pinned branch (`workspace::bundle_branch`). That is RUS197's area, and I flag it to them.

I check every PR with `cargo fmt --check`, `cargo clippy -p <crate> --all-targets -- -D warnings` on the changed crates, `cargo test -p <crate>` for each changed crate, and `bash scripts/check_file_size.sh`.
