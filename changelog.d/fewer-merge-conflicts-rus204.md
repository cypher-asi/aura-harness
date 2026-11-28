### Parallel agents stop colliding in the changelog and the lockfile

##### Changed

- A change's changelog entry is now its own file in `changelog.d/`,
  named after its branch, not an edit at the top of `CHANGELOG.md`.
  `aura changelog` shows what the fragments make, and
  `aura changelog --write` puts them into `CHANGELOG.md` at release time.
  `git_ship` stops a branch that edits `CHANGELOG.md` in a repo with
  `changelog.d/` and says how to move the entry. Agents are told the same
  in their instructions.

##### Added

- When the merge queue merges main into an entry and the only conflicts
  are in `CHANGELOG.md`, `Cargo.lock` or `package-lock.json`, it resolves
  them itself and carries on: both sides' changelog entries are kept, and
  a lockfile takes main's version and is brought up to date by cargo or
  npm. The build and tests still run on the commit that lands. A conflict
  in any other file still goes back to the agent, with the real files
  listed first.
- The merge queue records which files each entry changes. When another
  queued entry, a branch being fixed after a failed merge, or a channel
  member's worktree changes the same files, the QUEUE tab shows it under
  the entry ("Overlaps") and both agents are told once, so they can
  agree before both land. Nothing is blocked.
