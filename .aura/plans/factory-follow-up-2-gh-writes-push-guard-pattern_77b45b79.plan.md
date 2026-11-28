---
name: 'Factory follow-up 2: gh writes, push-guard patterns, REV69 nits'
overview: '#137 (19e1c2a9) and the fae0b601 doc update (q58, 423f14e5) are already on main, and no PR is open. What''s left is REV69''s remaining items: declare and police `gh`, add the cheap push-guard patterns, and fix the five nits. They ship as one PR through the queue.'
todos:
- id: gh-writes
  content: Declare gh's writes in forge/gh.rs and docs/invariants.md; add the gh band to check_invariants.sh
  status: pending
- id: guard
  content: 'Push guard: refuse gh api ref PATCH, /merges, graphql mergePullRequest, curl writes; tests'
  status: pending
- id: nits-branches
  content: refuse_unpushed returns rev-list errors; follow_up_branch checks the pin before checkout; tests
  status: pending
- id: nits-forge
  content: 401/403 fail clearly; already_landed closes the PR and deletes the branch (Forge::close); tests
  status: pending
- id: nits-broken
  content: Park at once on a known-broken tip with the same failing tests; test
  status: pending
- id: rules-docs
  content: FACTORY_RULES 'when you have it'; docs and changelog fragment
  status: pending
- id: ship
  content: Run the checks, ship through the queue, open the PR, report the PR and merge commit
  status: pending
---

# Factory follow-up 2: gh writes, push-guard patterns, REV69 nits

#137 (19e1c2a9) and the fae0b601 doc update (q58, 423f14e5) are already on main, and no PR is open. What's left is REV69's remaining items: declare and police `gh`, add the cheap push-guard patterns, and fix the five nits. They ship as one PR through the queue.

## Todos

- [ ] Declare gh's writes in forge/gh.rs and docs/invariants.md; add the gh band to check_invariants.sh
- [ ] Push guard: refuse gh api ref PATCH, /merges, graphql mergePullRequest, curl writes; tests
- [ ] refuse_unpushed returns rev-list errors; follow_up_branch checks the pin before checkout; tests
- [ ] 401/403 fail clearly; already_landed closes the PR and deletes the branch (Forge::close); tests
- [ ] Park at once on a known-broken tip with the same failing tests; test
- [ ] FACTORY_RULES 'when you have it'; docs and changelog fragment
- [ ] Run the checks, ship through the queue, open the PR, report the PR and merge commit

<!-- end of generated header: edit below -->

## Where things stand

I checked GitHub and main:

- **#137 is merged**, merge commit `19e1c2a9`.
- **`fae0b601` is merged**: the queue landed it as entry q58, merge commit `423f14e5`.
- **RUS204's #138 is merged**, merge commit `4849e5d2`.
- **No PR is open** on GitHub.

The leftover queue entry q20 that RUS205 mentioned lives in the hub's queue file, `<git-common-dir>/aura/merge-queue.json`. That file is on the hub, and this cloud machine can't read or edit it. Its branch is fully merged, so the entry is harmless: the queue marks it "already on the target" or expires it after 24 hours. I'll say so in the report rather than reach into the hub.

## The PR

The PR is built on the same branch, `aura/improve-the-software-factory-rus207`, because this machine still runs the old one-branch push rule. It starts from latest main and goes through `git_ship` and the queue, with no hand merge.

**Item 4, `gh`:**

- In `crates/exec/aura-exec-merge-queue/src/forge/gh.rs`, the module doc will declare everything `gh` writes on GitHub: open a PR, comment, set a commit status. It no longer merges, since #137 made the queue push to main instead. The now-unused `Forge::merge` stays for the trait, documented as unused by the landing path.
- `docs/invariants.md` gets the same declaration.
- `scripts/check_invariants.sh` gets a new band. It matches `Command::new("gh")`, and also `Command::new(` followed by a variable, in files that mention `gh` (a `program` field defaulting to `"gh"`). Only `forge/gh.rs` and tests are allowed. If the regex for the variable case gets awkward, the band will instead match the string literal `"gh"` used as a program default. The script must pass on the tree afterwards.

**Item 5, push guard** (`fs_tools/cmd/push_guard.rs`). New refusals, each with a test:

- `gh api` that PATCHes or POSTs `.../git/refs/...`;
- `gh api` on `.../merges`;
- `gh api graphql` whose words contain `mergePullRequest` or `enablePullRequestAutoMerge`;
- `curl` aimed at `api.github.com` with a write method (`-X PUT/POST/PATCH/DELETE`, or `--request`) on `/pulls/<n>/merge`, `/merges` or `/git/refs`.

Read-only calls stay allowed, and tests cover them too. Branch protection remains the real guard, and the doc says so.

**Nits:**

- **`refuse_unpushed`** (`git_tool/branches.rs`): a failed `rev-list` is returned as the error, not read as "nothing unpushed". Test: a repo with no `origin` remote refs still starts, while a broken `rev-list` refuses.
- **`follow_up_branch`**: the new branch name is checked against the pin before `git checkout -b`, so a refused name leaves the workspace on its branch. This moves the check from `ship.rs::follow_up` into `follow_up_branch` through a pin argument. Test: with a pin of another code, ship refuses and the current branch is unchanged.
- **401/403** (`forge/gh.rs::classify`): bad or missing credentials become `ForgeError::Failed` with a clear message ("GitHub refused the queue's credentials (401/403): sign in with gh auth login or set a token"), not `Unavailable`. Network errors and a missing `gh` still hold the queue. The forge's own error tests cover this. Whether this failure should fail the entry or hold the queue I still need to settle; the plan defaults to failing it with the clear message, as COO2 asked.
- **`already_landed`** (`integrate/pull_request.rs`): when the head is already on main and the PR is still open, the queue closes the PR with a comment ("already on main at <sha>") and deletes the branch with the lease. This needs a `Forge::close` method, implemented in `GhForge` (`gh pr close`) and the fake. Test in `tests/pull_requests.rs`.
- **Known-broken tip** (`integrate/diagnose.rs`): the integrator remembers the tip it found broken and the tests that failed there. When an entry fails the same tests on the same target tip, it parks at once and skips the two reruns. Test in `tests/broken_target.rs`: a second entry on the same broken tip parks with only one run of the check script, counted through a marker file.
- **FACTORY_RULES**: say "call `git_start_task` (when you have it)".

**Docs:** `docs/software-factory.md` gets the push-guard and `gh` lines. A changelog fragment goes to `changelog.d/improve-the-software-factory-rus207.md`.

## Checks before shipping

- `cargo test` for `aura-exec-merge-queue`, `aura-exec-tools`, and `aura-app factory`;
- `bash scripts/check_invariants.sh`;
- `bash scripts/check_file_size.sh`;
- fmt, and clippy on the touched files.

Then `git_ship`, open the PR, and wait for the queue to land it. Report the PR and its merge commit.
