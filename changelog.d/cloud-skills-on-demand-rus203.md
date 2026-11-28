### Skills load on demand on cloud machines too

##### Changed

- A cloud agent's turns no longer carry its skills' full text.
  - Before a turn, the app sends the agent's cloud machine its skill folders (each skill's `SKILL.md` and `references/` files, size-capped), but only when they have changed.
  - The machine lists the skills, and the agent loads one with `load_skill` when a task needs it.
  - A machine that hasn't been updated yet gets the full text, as before.
- `load_skill` can now load a skill's reference files: a loaded skill lists them, and `file` (for example `references/api.md`) returns one. This works for local agents too.
