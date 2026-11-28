# Software factory review

Where the plan, build and ship flow and the merge queue
(`crates/exec/aura-exec-merge-queue`) broke, conflicted or slowed down,
from PR #52 to #117 (27 to 28 November). Each finding has its evidence,
its root cause and its fix. They are ranked by cost: broken main first,
then rework, then time lost.

Evidence comes from `gh pr list --state all`, `git log --first-parent`
on `main` (at `90fa47f2`), the PR descriptions and the code. Durations
are from PR timestamps; where a cost is an estimate it says so.

The fixes for findings 1 to 3 land in the PRs that follow this review;
until each is on `main`, its "Fix" describes what it will do.

## Summary

| # | Finding | Cost | Fix |
|---|---|---|---|
| 1 | Nothing gates `main`: PRs and direct merges bypass the queue | main broken 4 times; 6 repair PRs | The queue lands through PRs, pinned to the tested head; branch protection |
| 2 | One branch per agent, reused for every task | 9 PRs on one branch, `-s ours` supersede merges, a stale head merged | One fresh branch per task; machines may push only their own agent's branches |
| 3 | A broken main is blamed on the next entry | innocent agents bounced; main red on every machine | Rerun a failing test on the plain target and hold instead of failing |
| 4 | The same feature built two or three times | about 10 PRs of duplicate or superseded work | Partly fixed (#71, #74, #83, #90); overlap warnings; the coordinator checks before reassigning |
| 5 | Conflicts: changelog, shared functions, hand resolutions that never compiled | #99 and #100; repeated merges of `main` | Fragments (#84); the queue builds every merge itself; conflicts fixed on the PR's own branch |
| 6 | Slowdowns: full rebuilds on every move of `main`, waits, lost reports | about 80 merges of `main` into branches in 7 hours | One checked run per PR; reports kept (#60, #85, #96) |
| 7 | Cloud start failures | every new agent failed for hours | RUS198 (see below) |
| 8 | Unreliable checks: clippy on old errors, flaky tests, Actions locked | file-size and flaky-test PRs (#98, #105, #110, #115) | The queue runs build and test only; a flaky test retried once |

## 1. Nothing gates `main`

**Evidence.**

- No commit on `main` says "through the merge queue". Since #52, every
  change on `main` is a GitHub "Merge pull request #N" (59 by the
  `cypher-asi-aura[bot]` App, 5 by the user) or a commit pushed directly.
  The queue ran beside GitHub, never in front of it: an entry it failed,
  like RUS203's at `8a1be374`, had often been merged as a PR already.
- The user's local agent merged into `main` directly four times
  (`209d774f`, `ff244c14`, `58667928`, `f443075a`, "Merge branch 'main'
  of https://github.com/..."). Those merges were never built:
  - `aura-app` stopped compiling: `missing field run_started` in
    `cloud/follow.rs`, because the direct merge of "Keep a cloud agent's
    turn going" (`5e9bd0e3`) met #96 and #97. #99 repaired it. Every cloud
    start in that window failed, since machines build from `main`.
  - `bbf6f9b7` put a changelog entry back into `CHANGELOG.md` that was
    already a fragment. #100 removed the duplicate.
  - The code came in unreviewed.
- #84 added the `changelog` subcommand outside the queue. Its CLI
  `--help` snapshot was not updated, so `cli_golden_top_level_help`
  failed on `main` for every entry until #109.
- Every merge is made as the same App bot, so GitHub can't say which
  agent merged a PR.

**Cost.** Main was broken four times (#91, #99, #100, #109 repaired it),
and every cloud start in that window failed. Six PRs and about two hours
of agents' time went to repairs.

**Root cause.** The queue's last step was a `git push` to `main`, but
agents opened PRs and something (agents with `gh pr merge` or `gh api`,
or the user) merged them first. Nothing on GitHub requires the queue's
check, so any merge path skips it. #81 blocked a typed `gh pr merge` in
`run_command`, but not `gh api -X PUT .../pulls/N/merge`, the merge
button, or a local merge pushed by the user.

**Fix.**

- The queue lands through the PR: it opens or finds the branch's PR,
  merges in the latest target, builds and tests that, pushes the tested
  merge to the PR's branch, and merges with
  `gh pr merge --merge --match-head-commit <tested sha>`. A problem
  leaves the PR open with a comment. The queue sets the
  `aura/merge-queue` commit status on the head it tested.
- The push guard also refuses `gh api` calls on `pulls/<n>/merge`.
- **The user must turn on branch protection** on `main` that requires
  the `aura/merge-queue` status, so a merge by hand can't skip the
  checks:

  ```sh
  gh api -X PUT repos/cypher-asi/aura-harness/branches/main/protection \
    -F 'required_status_checks[strict]=false' \
    -F 'required_status_checks[contexts][]=aura/merge-queue' \
    -F 'enforce_admins=false' -F 'required_pull_request_reviews=null' \
    -F 'restrictions=null'
  ```

  The App token on the machines can't read or set branch protection
  (403), so only the repo owner can run this.
- The user stops merging into `main` locally. A local change goes on a
  branch and through a PR like an agent's.

## 2. One branch per agent, reused for every task

**Evidence.**

- `aura/software-factory-rus197` backs 9 PRs, `owed-reports-survive-
  restart-rus203` 8, `why-members-stall-rus202` 7 and
  `fewer-merge-conflicts-rus204` 6.
- Unrelated work rides on old names: #111 ("Fold a running turn's
  steps") came from `channel-history-tab-rus200`, and #113 (RunPod) from
  `encrypt-local-database-rus205`. That branch is now fully merged
  (`4c5cc54a`, through #93 and #113; nothing on it is missing from
  `main`), but the queue still held a stale entry for an older tip
  (`26834d41`) reporting a `docs/integrations.md` conflict that #113
  had already resolved. A stale entry outlives the branch it named.
- Work that lives only on a machine is at risk: RUS205's workspace
  keeps a `git stash` of changes that predate its task and belong to
  no one on the channel. Resetting the workspace would lose them.
- To drop superseded history from a reused branch, agents made
  `-s ours` merges: `913e5eaf` ("Supersede the old HISTORY tab branch"),
  `d5db3396` and `02816ac1` (#72, #78 and #111 carry them).
- A stale head was merged: #84 merged at `372e9521`. Its follow-up commit
  `b74c4bb9` was pushed seconds before the merge, and GitHub merged the
  head it already had. The test fix never reached `main` until #91.

**Cost.** Two broken-main repairs (#91, and part of #109) and three
supersede merges. Every later PR on a reused branch carries its old
history, which makes review and conflict resolution harder. RUS205's
RunPod work had to wait on a branch named for other work.

**Root cause.** A cloud machine is pinned to one branch:
`CloudNode::git_branch()` goes into `ToolConfig::push_branch`, and
`resolve_branch` (`git_tool/push.rs`) refuses every other name. The
machine keeps that branch for its life, so each new task builds on the
last. A merge without `--match-head-commit` merges whatever head GitHub
has.

**Fix.**

- A new `git_start_task { task }` tool makes `aura/<task-slug>-<code>` at
  the latest `origin/<target>`, never reusing a name (`-2`, `-3`). It
  refuses when uncommitted or unpushed work would be left behind.
- A machine may push any `aura/<slug>-<its own code>` branch, and never
  the target or another agent's branch.
- A ship whose branch was already merged moves the new commits to a fresh
  branch before pushing.
- The queue merges pinned to the head it tested, and deletes the branch
  on origin after a confirmed merge or an abandoned PR.

## 3. A broken main is blamed on the next entry

**Evidence.**

- RUS203's entry at `8a1be374` failed `cargo test --workspace` on the
  insta snapshot of the CLI `--help`, which was missing the `changelog`
  subcommand #84 had added outside the queue. Main was red for every
  entry, and the queue failed whichever came next, which sent that agent
  to fix someone else's break (#109 did).
- It happened again while this review was in the queue. Its own entry
  (`3e461ec4`) failed on three Landlock confinement tests in
  `aura-exec-tools` (`confine::tests::*` and
  `test_confined_run_command_cannot_write_above_the_workspace`). The
  branch touches none of them: the tests picked an "outside" folder the
  hub could still write to, and failed on `main` itself until #129 fixed
  them. The queue bounced an innocent entry and told its agent to fix
  it, which is the case the broken-target check is built for.
- `cargo test --workspace` on `main` fails two targets on every cloud
  machine (`-p aura --test e2e_full` and `--test e2e_live`: 503 where 400
  is expected) and passes on the hub. `tests/common/server.rs` built its
  test server from `NodeConfig::from_env()`, so a machine's own
  `AURA_CLOUD_NODE=1` and `AURA_WORKSPACE` turned the test server into a
  cloud node. Every agent that ran the workspace tests saw a red main
  that wasn't.
- Flaky tests: two `/proc` scan tests raced under load and were fixed
  one after the other (#98, #105).

**Cost.** Agents bounced and debugging other agents' breaks. With the
env leak, every agent that ran the full tests lost time on 111 failures,
or learned to ignore failures, which hides real ones.

**Root cause.** The queue treats any failed check as the entry's fault.
It never checks whether the target fails the same way.

**Fix.**

- This review's PR makes the test server ignore the machine's node
  settings, and ignores the `*.snap.new` files a failed insta run leaves,
  so `add -A` can't commit them.
- When a test check fails, the queue reruns the failing tests once on the
  same commit; if they pass, the entry merges and the result names the
  flaky tests. If they fail again, it runs them on the plain target. If
  they fail there too, main is broken: the entry goes back in its place
  and isn't charged, the queue holds until the target moves, and the hub
  reports it to the channel's coordinator and to the agents whose merges
  since the last green target are suspects (a commit merged outside the
  queue is named as such).
- A change to the CLI's arguments or help can't ship without its updated
  snapshots: `git_ship` runs the CLI golden tests before pushing when
  those files change, and the checks run with `INSTA_UPDATE=no`.

## 4. The same feature built two or three times

**Evidence.**

- The HISTORY tab: #68 (RUS197), #72, #77 and #111 (RUS200), and #73 and
  #78 (RUS201). #73 was closed two minutes after it opened.
- The empty channel's welcome banner: #66 (RUS198) and #67 (RUS199,
  closed).
- The QUEUE tab's history: #87 (closed) and #80.
- The hub's leftover work: #57, #58 and #59 (closed snapshots), then #60
  and #61.
- The flaky-test pair, fixed in two PRs by two agents (#98, #105).

**Cost.** About 10 PRs of duplicate or superseded work, and the review
and merge time around them.

**Root cause.** Late starts (an assignment held while a machine
started), messages that arrived after the work had moved on, and
stand-downs that didn't cancel queued work. Agents also couldn't reach
each other by code, so they couldn't settle overlaps.

**Fixed already.** #71 (never start work taken back), #74 and #83 (mail
through the inbox, one run per agent, stalls reported) and #90 (reach
peers by code). The queue's overlap warnings (#84) tell two agents when
their open work changes the same files.

**Still open.** Nothing stops a coordinator from assigning the same
feature twice. The coordinator should check the open PRs and the QUEUE
tab before reassigning, and close a superseded PR in the same step.

## 5. Conflicts

**Evidence.**

- `CHANGELOG.md` conflicted on nearly every pair of parallel branches
  until #84 moved entries to `changelog.d/` fragments.
- Agents edited the same functions, for example the cloud follow code in
  #96, #97 and the direct merge that #99 repaired.
- Hand-resolved conflicts that never compiled: the direct merge behind
  #99 was resolved by hand and pushed without a build.
- There are about 80 "Merge remote-tracking branch 'origin/main'"
  commits on branches since #52: every ship merged the latest `main`, and
  many branches did so several times.

**Root cause.** Conflicts were resolved where nothing built them (a local
merge, or the agent's merge before a GitHub merge), and parallel
branches touched the same hot files.

**Fix.** Fragments (#84) and the queue's mechanical resolution of
generated files stay. Under the PR-based queue every merge that reaches
`main` is the one the queue built and tested. A conflict leaves the PR
open with the files named, and the agent fixes it on that PR's own
branch.

## 6. Slowdowns

**Evidence.**

- A full `cargo test --workspace` takes about 4 minutes with a warm
  build on a 4-core machine (7,168 tests), and much longer cold. With
  about 80 merges of `main` into branches in 7 hours, many of those runs
  were repeats of the same code.
- Agents waited on each other's fixes to `main` (#91, #99, #109).
- Lost and late reports: a stopped turn lost its report (#60), owed
  reports didn't survive a restart (#85, #96), and mail was lost while
  the hub was down (#94).
- Machines pinned to one branch forced the supersede merges of
  finding 2.

**Fix.** The queue tests each PR once, at the moment it lands, and a
moved head is retested before it merges. A broken main holds the queue
instead of failing entries one after another. The report fixes are on
`main` already (#60, #85, #96).

## 7. Cloud start failures

**Evidence.** Snapshots for new machines were built from the hub's local
commit, which was stale. While `main` didn't compile (before #99), every
new agent's start failed after about 12 minutes, and each message queued
for that agent started another failing start. The start panel showed
only the build's warnings, not its `error:` lines.

**Owner.** RUS198 is fixing this (snapshot from `origin/main`, the first
errors on the start panel, no restart per queued message). #79 already
starts a member's machine from origin's `main`.

## 8. Unreliable checks

**Evidence.**

- `cargo clippy --all-targets -- -D warnings` fails on errors older than
  any change, so agents stopped trusting it.
- The 500-line file limit and the 50-line function limit landed after
  most of the code; #110 and #115 split files to get under it.
- GitHub Actions are locked by billing, so no CI runs on PRs.

**Fix.** The user asked to ignore CI. The queue runs the committed
`.aura/merge-queue.toml` checks (build and test) and nothing else, a
flaky test gets one retry, and agents keep clippy clean on the files
they touch.

## Left out, and why

- **Testing only the affected crates.** Workspace-wide tests such as
  `every_crate_carries_a_matching_layer_doc_tag` and `cli_golden` would
  be skipped, and those are the tests that went red on `main`.
- **A shared build cache.** The queue's worktree already keeps its
  `target/` between entries.
- **A clippy baseline.** The queue doesn't run clippy, and the user said
  to ignore CI.
- **File claims at plan time.** They would serialize parallel work, and
  the overlap warnings already make agents talk.
- **GitHub's own merge queue.** It needs Actions, which are locked.

## What changes for agents and the user

- Start every task with `git_start_task`; never reuse a branch.
- Ship with `git_ship`. The queue opens the PR, tests it and merges it.
  Never merge a PR yourself.
- A failed entry's PR stays open with the reason; fix it on that branch
  and ship again.
- The user turns on branch protection (above), merges nothing locally,
  and rebuilds and restarts the hub so it runs the new queue.
