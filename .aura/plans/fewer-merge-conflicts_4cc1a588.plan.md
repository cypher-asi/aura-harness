---
name: Fewer merge conflicts
overview: Stop parallel agents from colliding in CHANGELOG.md. Each change writes its own changelog.d fragment, assembled at release time. The merge queue resolves conflicts that are only in CHANGELOG.md or lockfiles itself and still sends real code conflicts back with the file list. When two pieces of work touch the same files, the queue records it, shows it on the Queue tab and tells both owners, without blocking anything.
todos:
- id: hygiene
  content: Back up the stray local commits and changes (branch + stash), reset the branch to origin/main
  status: pending
- id: coordinate
  content: Fold RUS199/RUS200/RUS197 replies into hook points and merge order
  status: pending
- id: changelog-mod
  content: 'Queue crate: changelog.rs fragment assembly with unit tests'
  status: pending
- id: cli
  content: 'aura-cli: `aura changelog [--write]` subcommand'
  status: pending
- id: fragments-repo
  content: Add changelog.d/README.md and this PR's own fragment
  status: pending
- id: ship-guard
  content: 'aura-exec-tools: git_ship fragment guard (fragments.rs) with tests'
  status: pending
- id: prompts-docs
  content: FACTORY_RULES, cloud_git_note, docs/development.md, docs/software-factory.md
  status: pending
- id: resolve
  content: 'Queue crate: resolve.rs auto-resolution hooked into merge_commit; git allowlist'
  status: pending
- id: conflict-tests
  content: 'tests/conflicts.rs: CHANGELOG-only, code conflict, Cargo.lock'
  status: pending
- id: overlap-core
  content: 'Queue crate: MergeEntry.files, record_files, overlap.rs (changed_files, open_work, find) with tests'
  status: pending
- id: overlap-app
  content: 'aura-app factory/overlap.rs: record files, find active worktrees, notify both owners once'
  status: pending
- id: overlap-ui
  content: MergeEntryView.overlaps, view.rs fill, channel_queue.rs Overlaps fact, tests
  status: pending
- id: gates
  content: fmt, clippy -D warnings, tests for changed crates, file size check
  status: pending
- id: ship
  content: Push branch, open PR (note last CHANGELOG conflict), merge main, retest, merge commit, report
  status: pending
---

# Fewer merge conflicts

Stop parallel agents from colliding in CHANGELOG.md. Each change writes its own changelog.d fragment, assembled at release time. The merge queue resolves conflicts that are only in CHANGELOG.md or lockfiles itself and still sends real code conflicts back with the file list. When two pieces of work touch the same files, the queue records it, shows it on the Queue tab and tells both owners, without blocking anything.

## Todos

- [ ] Back up the stray local commits and changes (branch + stash), reset the branch to origin/main
- [ ] Fold RUS199/RUS200/RUS197 replies into hook points and merge order
- [ ] Queue crate: changelog.rs fragment assembly with unit tests
- [ ] aura-cli: `aura changelog [--write]` subcommand
- [ ] Add changelog.d/README.md and this PR's own fragment
- [ ] aura-exec-tools: git_ship fragment guard (fragments.rs) with tests
- [ ] FACTORY_RULES, cloud_git_note, docs/development.md, docs/software-factory.md
- [ ] Queue crate: resolve.rs auto-resolution hooked into merge_commit; git allowlist
- [ ] tests/conflicts.rs: CHANGELOG-only, code conflict, Cargo.lock
- [ ] Queue crate: MergeEntry.files, record_files, overlap.rs (changed_files, open_work, find) with tests
- [ ] aura-app factory/overlap.rs: record files, find active worktrees, notify both owners once
- [ ] MergeEntryView.overlaps, view.rs fill, channel_queue.rs Overlaps fact, tests
- [ ] fmt, clippy -D warnings, tests for changed crates, file size check
- [ ] Push branch, open PR (note last CHANGELOG conflict), merge main, retest, merge commit, report

<!-- end of generated header: edit below -->

## Where things stand

In the last ~25 PRs almost every merge conflicted in `CHANGELOG.md`, because each PR adds its entry at the top of `[Unreleased]`. The merge queue (`crates/exec/aura-exec-merge-queue`) merges the target into each entry in its own worktree (`integrate.rs`, `merge_commit`). On any conflict it aborts and fails the entry with `conflict_reason`. The hub (`aura-app` `factory/service.rs`) turns that failure into a notice for the agent that pushed. Nothing records which files an entry changes, so nobody hears that two agents are editing the same code until one of them conflicts.

**Workspace hygiene first.** This clone's branch has two local commits that aren't on origin ("Set a reply's paragraphs apart…", "List TASKS by day…") and 91 files of uncommitted changes that aren't mine. `git_ship` commits everything, so before building I keep that work and move it aside. I tag HEAD as the local branch `rus204-preexisting-backup` and run `git stash push -u` with a message. Then I reset this branch to `origin/main`. I'll report it to COO2 and won't ship it.

## Coordination

Before planning I messaged RUS199 (queue target, worktree location), RUS200 (Queue tab history, late-open state) and RUS197 (raw push blocking, per-member worktrees). I proposed the file split below and asked about merge order. Their answers can move a hook point, but not the design. The rule throughout: new logic goes in **new files**, and shared files get small hooks.

| Shared file | My change |
|---|---|
| queue `integrate.rs` | The conflict branch of `merge_commit` calls `resolve`. `conflict_reason` moves out, so the file stays under 500 lines. |
| queue `git.rs` | `add`, `commit` and `merge-file` are added to `ALLOWED_SUBCOMMANDS`. |
| queue `entry.rs`, `state.rs`, `processor.rs`, `lib.rs` | `MergeEntry.files`, `QueueState::record_files`, `MergeQueue::record_files`, and the module lines. |
| tools `git_tool/ship.rs`, `git_tool/mod.rs` | One call to the fragment guard between the pull and the push, and `mod fragments;`. |
| app `factory/service.rs`, `factory/view.rs`, `factory/mod.rs` | One call after the enqueue; the overlaps go into the view; one sentence in `FACTORY_RULES`. |
| ui `views/merge_queue.rs`, terminal `renderer/channel_queue.rs` | A `MergeEntryView.overlaps` field and an "Overlaps" fact. |
| node `chat/turn.rs` | One sentence in `cloud_git_note`. |

`config.rs` stays untouched because it's RUS199's. Whoever ships first goes first; I merge main in afterwards.

## 1. Changelog fragments, assembled at release time

I assemble at **release time**, not merge time, because that is safer. Merge-time assembly would mean the queue writes content into every landing commit, beyond what the agent pushed. Release-time assembly keeps the queue's rule intact: what lands is the agent's merge, checked.

- `changelog.d/README.md` explains the convention. A change adds `changelog.d/<last segment of its branch>.md`, for example `fewer-merge-conflicts-rus204.md`. Its content is the usual block: `### Title`, a blank line, `##### Fixed|Added|Changed`, then bullets.
- The new `aura-exec-merge-queue::changelog` module is pure. `assemble(changelog, fragments)` puts the fragments, newest first, right under `## [Unreleased]`. It fails clearly when there is no `[Unreleased]` heading, a fragment is empty, or a fragment doesn't start with `### `. `Fragment::read_dir` skips `README.md`.
- A new `aura changelog` subcommand in `aura-cli` previews the assembled `[Unreleased]` section. `aura changelog --write` writes `CHANGELOG.md` and deletes the fragments. Fragments are ordered by when git added them (`git log --diff-filter=A`); uncommitted ones count as newest.
- **The ship flow:** a new `git_tool/fragments.rs` runs after `git_ship`'s pull. It checks whether the repo has `changelog.d/` and the branch's diff against the fetched target (`FETCH_HEAD..HEAD`) edits `CHANGELOG.md` without deleting a fragment, which would make it a release. If so, `git_ship` stops before pushing. It tells the agent to move the entry into `changelog.d/<slug>.md` and restore `CHANGELOG.md` from main. Repos without `changelog.d/` behave as before.
- **The prompts:** `FACTORY_RULES` and `cloud_git_note` both say: "If the repo has a `changelog.d/` folder, write your changelog entry as a new file there, named after your branch; never edit CHANGELOG.md."
- **The docs:** the new flow goes in `docs/development.md` (replacing "Release notes go in CHANGELOG.md") and in a new section of `docs/software-factory.md`. This PR's own entry is a fragment.

## 2. The queue resolves trivial conflicts

A new `resolve.rs` holds a built-in table of generated files, matched by file name at any depth:

| File | Resolution |
|---|---|
| `CHANGELOG.md` | Union: `git merge-file --union` over stages 1, 2 and 3, written to temp files in the queue dir, so both sides' new entries stay. |
| `Cargo.lock` | The target's side (`checkout --ours`, since the queue merges into the target), then `cargo metadata --format-version 1` in its folder adds the branch's dependencies. |
| `package-lock.json` | The target's side, then `npm install --package-lock-only --ignore-scripts`. |

When `merge_commit`'s merge fails, `resolve::try_resolve` gets the conflicted files.

- **All generated:** it resolves each file, runs `git add` and then `git commit --no-edit`, which keeps the queue's merge message. The merge goes on to the checks as usual, so the build and tests still run on exactly the commit that lands. The note adds "resolved CHANGELOG.md mechanically".
- **Any real file:** it runs `merge --abort` and fails the entry. The reason lists the real files first and says the generated ones would have been resolved. `failure_prompt` already sends that reason back to the owning agent, telling it to pull main, resolve and re-ship.
- **Not mechanical:** a modify/delete on a generated file, or a regenerate command that fails, counts as a real conflict, and the reason says why.

## 3. Overlap warnings, without blocking anything

- `MergeEntry` gains `files: Vec<String>` (serde default). `overlap::changed_files` works them out at enqueue. For a remote entry it fetches the target and the head into private `refs/aura-queue/scan/<id>/…` refs, which are deleted afterwards. Then it runs `git diff --name-only <target>...<head>`. Generated files are left out of overlap.
- `overlap::open_work(entries)` lists the work in flight: entries that are queued or running, plus each branch whose latest entry failed and hasn't merged since (its agent is fixing it). `overlap::find(subject, others)` returns each other piece of work that shares files, with the shared files. Both are pure.
- **Active branches** also include channel members' local worktrees on a branch other than the target. The new app `factory/overlap.rs` finds them from the registry's agent folders in the same repo and runs `git diff --name-only <merge-base>`, which covers committed and uncommitted changes. Cloud members' work counts once it is pushed (then it is an entry); I'll document that limit.
- **After an enqueue,** `factory/overlap.rs` records the files (`MergeQueue::record_files`, which applies only while the head matches). It then finds the overlaps and sends a `Notice` to both owners. Each notice names the other agent, its branch and entry, and the shared files, and tells them to coordinate with `send_to_agent` before integrating. Pairs already told are remembered in memory, so a re-push with the same overlap doesn't repeat the notice.
- **On the Queue tab,** `MergeEntryView.overlaps` holds lines like "q12 aura/x (RUS199): src/a.rs, src/b.rs". The terminal shows them as an "Overlaps" fact under the entry.

## Tests

- **Queue crate unit tests:** changelog assembly (ordering, README skipped, missing `[Unreleased]`, bad fragment), which files count as generated, the conflict reason listing only real files, `record_files`, `open_work` and `find`.
- **`tests/conflicts.rs`** (real git, bare origin and clones):
  - a CHANGELOG-only conflict merges, keeps both entries and leaves no conflict markers;
  - a code conflict together with a CHANGELOG conflict fails with `src` files named;
  - a Cargo.lock conflict is resolved with cargo.
- **`tests/overlap.rs`:** `changed_files` for a local branch and for a remote one.
- **Tools:** the fragment guard stops a ship that edits `CHANGELOG.md` when `changelog.d/` exists, and lets it through otherwise or when fragments are deleted.
- **App:** the view shows overlaps, and the overlap notices go to both owners, only once.
- **CLI:** parsing for `aura changelog`.
- **Gates:** `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` for aura-exec-merge-queue, aura-exec-tools, aura-app, aura-ui, aura-ui-terminal, aura-node and aura-cli, plus `scripts/check_file_size.sh`.

## Shipping

The PR goes from this branch, based on the latest `origin/main`, pushed without force. Its description says that in-flight branches that already edited `CHANGELOG.md` conflict one last time. If they're queued, the queue now resolves that itself. On their next `git_ship`, the guard stops them until the entry is moved to a fragment. Then I merge main in, rerun the tests, and merge with a merge commit through `git_ship` and the queue, or `gh pr merge --merge` if the queue isn't the route. Finally I report the PR link, the merge commit and what agents now do differently.
