# Changelog fragments

Don't edit `CHANGELOG.md` for a change. Add one file here instead, named
after your branch's last segment: branch `aura/fix-scroll-rus12` writes
`changelog.d/fix-scroll-rus12.md`. No other branch writes that file, so
changelog entries no longer conflict when parallel branches merge.

A fragment holds one entry, as it would read under `## [Unreleased]`:

```markdown
### What changed, as a title

##### Fixed

- What the user sees now, and what they saw before.
```

The level-5 heading is one of `Added`, `Changed`, `Fixed`, `Removed` or
`Security`. A change with more than one kind of note uses more than one.

At release time, `aura changelog` shows the entries the fragments make,
newest first (by when git added each fragment). `aura changelog --write`
puts them at the top of `[Unreleased]` in `CHANGELOG.md` and deletes the
fragments; commit both in one commit.

`git_ship` stops before pushing a branch that edits `CHANGELOG.md` in a
repo with this folder, unless the same branch deletes fragments (a
release). See `docs/development.md`.

This README is not a fragment; neither is any file that doesn't end in
`.md`.
