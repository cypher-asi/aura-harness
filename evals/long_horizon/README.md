# Long-horizon evals

This suite measures how well `aura -p` finishes multi-file feature work on its own. Each task is a small project with working tests, a feature request that touches several files, and hidden tests that the agent never sees. The runner gives the agent the request in a scratch copy of the project, then copies the hidden tests in and runs the project's test command. A task passes when every test, hidden and visible, passes.

Alongside pass/fail, the report records what the run cost: model calls, tokens, prompt cache hit rate, estimated cost at API list prices, history compactions, open todo items, and wall time. These come from the `json` result that `aura -p` prints (see [docs/cli.md](../../docs/cli.md#output-formats)).

## Tasks

| Task | Language | What the agent builds |
|------|----------|-----------------------|
| `py-inventory-csv` | Python | CSV import/export with all-or-nothing validation, a per-category value report, and three CLI commands. |
| `py-ratelimit` | Python | `retry_after` for the token bucket, a sliding-window limiter, a per-client registry that forgets idle clients, and limiters built from config. |
| `py-markdown-toc` | Python | Code-fence-aware heading parsing, GitHub-style unique slugs, a table-of-contents generator that rewrites a marked section, and a CLI command. |
| `rust-kv-ttl` | Rust | An injectable clock, key expiry with `EXPIRE`/`TTL`, and a session with `MULTI`/`EXEC`/`DISCARD` transactions. |

Each task directory holds:

- `task.json`: the id, title, prompt given to the agent, test command, where the hidden tests go, and the test timeout.
- `project/`: the starting project. Its own tests pass as shipped.
- `hidden_tests/`: tests copied into the project only for grading.
- `reference/`: files that, copied over the project, make every test pass. Only `--self-check` uses them.

The Python tasks need only `python3` (3.8 or newer); the Rust task needs `cargo`. No task downloads anything.

## Running

Build `aura` first; the runner uses `target/release/aura`, then `target/debug/aura`, then `aura` on `PATH` (or `--aura PATH`).

```sh
cargo build --release

# Check the fixtures: no model is called. Safe for CI.
python3 evals/long_horizon/run.py --self-check

# Exercise the runner end to end for free. The mock model solves nothing,
# so every task fails, but the report shows the pipeline works.
python3 evals/long_horizon/run.py --provider mock

# A real run. Calls real models and costs money, so it needs --live
# (or AURA_EVALS_LIVE=1).
python3 evals/long_horizon/run.py --live --model claude/claude-sonnet-4-5 \
  --max-time 30m --max-cost-usd 5

# Some tasks only, keeping the scratch projects to inspect afterwards.
python3 evals/long_horizon/run.py --live --tasks py-ratelimit,rust-kv-ttl --keep
```

Without `--live` or `AURA_EVALS_LIVE=1`, any run that is not `--self-check` or `--provider mock` refuses to start, so CI never pays for model calls by accident. The model needs the usual sign-in (`aura login`, a linked plan, or a local Ollama model); see [docs/cli.md](../../docs/cli.md#sign-in-and-model-routes).

Options: `--tasks` (comma-separated ids), `--model`, `--provider`, `--mode`, `--max-turns`, `--max-time`, `--max-cost-usd` (all passed to `aura -p` per task), `--out DIR` (default `target/evals/long-horizon/<timestamp>/`), `--keep`, `--aura PATH`.

Each task runs in its own scratch directory under the system temp directory (`TMPDIR`), with its own `AURA_DATA_DIR` and its own `CARGO_TARGET_DIR`, so runs never share sessions or build artifacts. `aura -p` gets no stdin. The runner kills an `aura -p` that outlives `--max-time` by two minutes (two hours without `--max-time`). Its stderr is kept as `aura.stderr.log` in the scratch directory, which `--keep` preserves.

## Report

The runner writes `report.json` and `report.md` to the output directory. Per task:

| Field | Meaning |
|-------|---------|
| `passed` | Every test passed after the hidden tests were added. |
| `exit_reason` | From `aura -p`: `success`, `max_turns`, `max_time`, `max_cost`, `error`, `interrupted`; or `killed` / `no_result` when the runner got no result. |
| `aura_exit_code` | Exit code of `aura -p`. |
| `num_turns`, `agent_turns` | Model calls, and agent turns (the prompt plus background-job notices). |
| `cost_usd` | Estimated cost at API list prices (`null` when no call had a price). |
| `cache_hit_rate` | Cache reads over all prompt tokens. |
| `input_tokens`, `cache_read_tokens`, `cache_write_tokens`, `output_tokens` | Token totals. |
| `compactions` | History summaries during the run. |
| `todos_total`, `todos_open` | The agent's todo list at the end, and items still pending or in progress. |
| `files_changed`, `wall_time_s`, `session_id`, `error` | As reported by `aura -p`, plus the runner's wall time. |
| `test_output_tail` | The last 40 lines of the test run. |

The summary adds the pass rate, total cost, the token-weighted cache hit rate, and total compactions, model calls and wall time. `report.md` shows the same as a table, followed by the test output of each failed task.

## Adding a task

1. Create `tasks/<id>/project/` with a small project whose tests pass.
2. Write `task.json`. Spell out every name, signature, message and edge case the hidden tests check; the agent sees only the prompt and the project.
3. Put the hidden tests in `hidden_tests/`. Name them so they cannot collide with tests the agent writes (for example `test_hidden_*.py` or `hidden_*.rs`).
4. Write a reference solution in `reference/`, as files that replace or add to the project's.
5. Run `python3 evals/long_horizon/run.py --self-check --tasks <id>`. It checks that the shipped tests pass, that the hidden tests fail before the feature exists, and that the reference solution passes them all.
