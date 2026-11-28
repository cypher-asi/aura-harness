### A changelog entry can't be listed twice

##### Fixed

- "A reply's paragraphs are set apart" was both in `CHANGELOG.md` and in
  its `changelog.d/` fragment, so the next release would have listed it
  twice. It is now only in the fragment.

##### Added

- `aura changelog --check` fails when a `changelog.d/` fragment's entry
  is also under `[Unreleased]` in `CHANGELOG.md`, and names both. The
  preview and `aura changelog --write` refuse it too, and a repo test
  (`tests/changelog_fragments.rs`) fails on it, so the merge queue's
  checks catch an entry that a direct merge put back.
