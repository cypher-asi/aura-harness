# Coding eval suite

Small coding tasks with a verification command. Each run is recorded as an
`eval` run under `AURA_HOME/runs/`, with the verification result stored as
the run's outcome reward, so arms of harness parameters can be compared on
outcome, tokens, model calls, wasted reads and failed edits.

```sh
# Baseline with the default harness parameters.
aura eval run evals/coding --model <model>

# A candidate arm: one harness parameter changed.
aura eval run evals/coding --model <model> --set implement_now_threshold=4 --repeat 3

# Mean metrics per arm, per-task pass counts, and regressions against
# the first arm.
aura eval compare --suite coding --arm default --arm implement_now_threshold=4
```

`--provider mock` runs the suite against the offline mock model. That
checks the mechanics (workspace setup, recording, verification, scoring)
but not task success.

## Task format

Each task is a directory with a `task.toml` and an optional `workspace/`
fixture that is copied into a fresh directory (`AURA_HOME/evals/work/`
by default) before the run.

```toml
title = "Short title"
prompt = "The first user message."
follow_ups = ["Optional extra turns."]
verify = "shell command run in the workspace; exit 0 passes"
max_iterations = 30
```

Verification runs with `sh -c`. Keep it fast and deterministic and make it
check behavior, not wording.

## Tasks

| Task | What it measures |
|---|---|
| `write-file` | Following an exact instruction with one tool call. |
| `fix-shell-bug` | Locating and fixing an off-by-one from a failing check. |
| `rename-symbol` | Multi-file edits without missing a reference. |
| `rust-failing-test` | Read, edit, then run tests in a real crate. |
| `multi-turn-feature` | A feature built over three turns, each building on the last. |
