### The merge queue's test run no longer fails on the confinement tests

##### Fixed

- `cargo test --workspace` failed in the merge queue on three tests of
  confined commands, for every entry. They wrote "outside" their
  workspace into the folder the tests ran in, and checked that it was
  refused; but the queue checks entries in a worktree under the user's
  cache folder (`~/.cache/aura-merge-queue/…`), which confined commands
  may write to on purpose, so the write went through. The tests now pick
  a folder outside every writable root, and skip with a note when there
  is none.
