# Run traces, evals and training data

Every coding run can be recorded as a trajectory: an ordered list of steps, each holding what the decision-maker saw, what it chose, which policy chose it, and the rewards that followed. Two kinds of decision-maker are recorded the same way:

- **The model**: each LLM call. The observation is the request, and the action is the response (thinking, text, tool calls).
- **The harness**: each steering nudge, block, compaction, cache hit, retry, build check or budget stop. The observation is the trigger state, and the action is the choice plus the `HarnessParams` behind it.

One recording feeds the scorecards, the CLI and TUI views, eval comparisons and dataset exports. The code lives in [`aura-agent-trace`](../crates/aura-agent-trace); the schema is in [`aura-core-types/src/types/trace.rs`](../crates/aura-core-types/src/types/trace.rs).

## When runs are recorded

`AURA_TRACE` sets the capture level:

| Value | What is kept |
|-------|--------------|
| `off` (also `0`, `false`, `none`) | Nothing. |
| `decisions` (default; also `on`, `1`, `true`) | Steps, decisions, token usage and rewards. |
| `full` (also `blobs`) | Everything in `decisions`, plus the exact model requests, responses, thinking text and full tool outputs as content-addressed blobs. |

The terminal UI records every conversation at `decisions` unless `AURA_TRACE=off`. The gateway (`aura-node`) and automaton tasks record only when `AURA_TRACE` is set, so servers and tests don't write to `AURA_HOME` unasked. `aura eval run` and `aura runs branch` always record, at `decisions` or higher.

## Bundle layout

```
AURA_HOME/
  runs/<run_id>/
    meta.json       run metadata (below)
    steps.jsonl     one Step per line, in seq order
    rewards.jsonl   stored rewards (outcome, human labels, interrupts)
    summary.json    the RunScorecard, rewritten when the run ends or is relabeled
    record.jsonl    kernel record rows seen during the run (seq, tx type, tool_use_id)
    record/         RocksDB kernel record log (headless runs: eval, branch)
  blobs/            shared zstd-compressed blobs, addressed by the blake3 of the raw bytes
```

`meta.json` holds `schema_version`, `run_id`, start and end times, `harness_version`, `model`, `mode`, `source` (`tui`, `gateway`, `automaton`, `eval`, `branch`, `test`), `task_id`, `title`, `workspace`, `trace_level`, the full `HarnessParams` snapshot, `consent`, and for branches and eval arms `parent_run`, `branch_step`, `arm` and `arm_propensity`.

Blobs are shared across runs, so identical requests and outputs are stored once. The kernel uses the same directory for the full payloads behind summarised `AuditedLite` effects, which lets kernel replay resolve them.

## Steps

Each line of `steps.jsonl` is a `Step`:

| Field | Meaning |
|-------|---------|
| `seq` | Position in the run, from 1. |
| `parent` | Enclosing step, e.g. the model call a tool result answers. |
| `turn`, `iteration` | 1-based user turn and zero-based loop iteration. |
| `actor` | `user`, `model`, `harness` (with its `DecisionSource`) or `environment`. |
| `observation` | What the actor saw: a model request summary and blob refs, or the harness trigger features. |
| `action` | `user_prompt`, `model`, `harness`, `tool_result` or `turn_end`. |
| `policy` | Which policy acted (`model`, `harness`, `human`, `environment`), its version, params and propensity. |
| `links` | Join keys: tool-use ids, files touched, blob ids, and `record_seqs` into the kernel record log. |
| `usage`, `duration_ms` | Tokens and wall time for model and tool steps. |

Harness decisions name a `DecisionSource`: `steering`, `compaction`, `budget`, `tool_pipeline`, `cache`, `policy`, `retry`, `build`, `self_review`, `thinking` or `stop`. A test in `aura-agent/src/agent_loop/decisions.rs` fails if any source loses its emit site.

When a run is loaded, the steps are joined to `record.jsonl`: user prompts match by order, and tool calls and results match by `tool_use_id`. Each matched step gets `record_seqs`, so the graph view can jump from a step to its kernel record row.

## Rewards

A reward targets a step or the whole run and has a `kind`, a `name`, a `value` (positive is good), a versioned `source` and optional `evidence`.

| Kind | Where it comes from | Stored or derived |
|------|---------------------|-------------------|
| `outcome` | Verification command (`aura runs verify`, eval `verify`). | Stored |
| `human_explicit` | `aura runs label`, or `+`/`-` in the TUI graph. | Stored |
| `human_implicit` | Interrupts and similar user behavior. | Stored |
| `process` | Local progress: an edit applied, a build fixed, a redundant read. | Derived (`process/v1`) |
| `cost` | Tokens, money and time spent. | Derived (`cost/v1`) |
| `cost` (`child_cost_usd`) | In eval runs, money spent by gate workers on the run's behalf, from each worker's own usage. | Stored |
| `decision_effect` | Whether the model did what a harness decision intended within 3 model calls. | Derived (`effect/v1`) |

Derived rewards are recomputed from the steps every time a run is loaded, so improving a labeler relabels all past runs. The normalizer assigns credit with a discounted return (gamma 0.95).

## Scorecard

`RunScorecard` (in `summary.json`, `aura runs show` and `/status`) reports turns, model and tool calls, tool errors, input, output and cache tokens, wall and model time, time and tokens to first edit, reads and writes, redundant reads, failed edits, blocked calls, cache hits, retries, decisions and compactions by kind, per-decision effect rates, stop reasons, files changed, reward totals and the outcome.

## CLI

```sh
aura runs list [--limit N] [--json]
aura runs show <id> [--steps] [--json]          # id may be a unique prefix or suffix
aura runs diff <baseline> <candidate> [--tolerance 0.1]
aura runs label <id> <value> [--step N] [--name human] [--note "..."]
aura runs verify <id> --cmd "cargo test"
aura runs branch <id> --from-step N --workspace <dir> [--model M] [--set key=value] [--arm A]

aura eval run [SUITE] [--task T] [--repeat N] [--model M] [--set key=value] [--arm A] [--budget-usd USD]
aura eval compare [--suite S] [--arm A --arm B] [--tolerance 0.1] [--fail-on-regression]
                  [--baseline FILE] [--write-baseline FILE]

aura dataset build --format sft|preference|rl|harness-bandit [--filter EXPR] [--out DIR]
```

`--set` overrides one `HarnessParams` field, such as `implement_now_threshold=4`, `repeated_read_threshold=3` or `failed_edit_stop_after=4`. Unknown keys are rejected. The arm label defaults to the overrides joined together.

`aura runs branch` replays the model responses recorded before step N and runs step N onward live in `--workspace`, which must be in the state the original run started from (for example a fresh checkout). Tools run there for real. The branch records `parent_run` and `branch_step`, so a branch and its parent can form a preference pair.

## Datasets

`aura dataset build` writes `train.jsonl`, `eval.jsonl` and `dataset_card.json` to `AURA_HOME/datasets/<format>` by default.

| Format | One record per | Contents |
|--------|----------------|----------|
| `sft` | Successful run | Provider-neutral chat with tool calls (`role`, `content`, `tool_calls`, `tool_call_id`). Only runs with a positive outcome unless `--filter` sets one. |
| `preference` | Task with differing outcomes | `chosen` and `rejected` chats for the same task, including branches of one run. |
| `rl` | Step | Observation, action, reward, discounted return, done flag, policy and propensity. |
| `harness-bandit` | Harness decision | Features, action, params, propensity and measured effect. |

`--filter` takes comma-separated terms: `outcome>=0.5`, `model=<id>`, `source=tui`, `eval=include` (eval runs are excluded by default) and `eval_fraction=0.1`. Every export redacts secrets, skips runs without consent, splits train and eval by task so a task never appears on both sides, and drops duplicate records. With `AURA_TRACE=full`, `sft` and `preference` chats include full tool outputs from the blob store.

## Evals

[`evals/coding`](../evals/coding) is a suite of small coding tasks, each a `task.toml` (title, prompt, follow-ups, verify command, iteration cap) plus an optional `workspace/` fixture. `aura eval run` copies each fixture into a fresh directory, runs the task headless, runs the verify command, and stores the result as the run's outcome. See the [suite README](../evals/coding/README.md) for the task format.

A task may set `category` (default `general`) and `tags`, and may keep a `checks/` folder next to `workspace/`: it is copied into the workspace only after the agent finishes, so verify commands can run tests the agent never saw. The results table shows each run's cost, gate workers included, and `--budget-usd` stops starting runs once the suite has spent that much. [`evals/gate_cost`](../evals/gate_cost/README.md) uses both to measure the cost gate; `aura gate bench` compares its arms (see [Gate models](cli.md#gate-models)).

`aura eval compare` shows mean metrics per arm, per-task pass counts and regressions against the first arm, or against a baseline file with `--baseline`. `scripts/eval-live.sh` and the manually triggered `Live coding evals` workflow run the suite against a live model (`AURA_EVAL_MODEL`) and fail on regressions. `--provider mock` checks the mechanics offline; CI covers it in `tests/eval_suite.rs`.

## Improvement loop

1. **Observe.** Use the TUI (`/graph`, the TRACE tab, `/status`) or `aura runs show` to find where runs waste work: redundant reads, failed edits, ignored nudges, or late first edits.
2. **Change one thing.** Either a `HarnessParams` value (`--set`) or harness code.
3. **Measure.** Run `aura eval run --set ...` for the candidate arm next to the default arm, then `aura eval compare --fail-on-regression`. Check against the live baseline before merging harness changes.
4. **Counterfactuals.** Run `aura runs branch` from the step where a run went wrong, with another model or other params, to see whether that decision mattered.
5. **Label.** Use `aura runs verify` and `aura runs label` (or `+`/`-` in the graph) to add outcomes and human judgments.
6. **Export.** Run `aura dataset build` to train the model (`sft`, `preference`, `rl`) or tune the harness (`harness-bandit`).

## TUI

- `/graph [run id]` opens the graph of the current run, or of the given run, over the chat panel. `/runs` lists recorded runs, and `/status` shows the live scorecard.
- The graph is a tree of runs, turns, iterations and steps, with a metadata pane for the selected node.

  | Key | Action |
  |-----|--------|
  | `j`/`k` or arrows | Move. |
  | `h`/`l` or left/right | Collapse, or go to the parent; expand, or go to the first child. |
  | `Enter`/`Space` | Toggle the node. |
  | `Tab` | Follow the node's cross-link, e.g. tool call to result. |
  | `e` | Show only issues. |
  | `+`/`=` and `-` | Label the step good or bad; stored as a `human_explicit` reward. |
  | `r` | Jump to the step's row in the KERNEL record panel. |
  | `o` | Open the step's file. |
  | `g`/`G` | First and last node. |
  | `q`/`Esc` | Close the graph. |

- The **TRACE** side tab is a live feed of iterations, tool calls, decisions and rewards; its count is marked `!` when it contains issues. Selecting an entry opens it in the graph. Shift with the arrow keys cycles the side tabs.
