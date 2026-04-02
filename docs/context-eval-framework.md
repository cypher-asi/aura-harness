# Context Eval Framework

This document defines how we should evaluate harness improvements in a way that is honest, practical, and useful for product decisions.

The goal is not to prove that we added more telemetry.
The goal is to prove that the harness creates real value.

In simple terms, the harness should help us answer four questions:

1. Is it cheaper?
2. Is it faster?
3. Is it more reliable?
4. Is the output still good?

If the answer is not improving in at least one meaningful dimension without hurting the others, then the optimization is not worth much.

## Why Raw Token Counts Are Not Enough

Raw token counts can be misleading.

A cache-aware run can show:

- more total prompt-footprint tokens
- more cache tokens
- slightly different billed input/output numbers

and still be a win.

That is because the real question is not:

- "Did the prompt touch more tokens?"

The real questions are:

- "What did we actually pay for?"
- "Did it finish faster?"
- "Did it fail less?"
- "Did it still do the job well?"

So our evals need to separate:

- billed usage
- cache activity
- context occupancy
- latency
- success
- quality

## The Four Buckets

## 1. Cost

This bucket answers:

- Did the harness reduce effective cost per successful task?
- Did caching and compaction reduce expensive repeated prompt work?

### Core metrics

- `billed_input_tokens`
- `billed_output_tokens`
- `cache_write_tokens`
- `cache_read_tokens`
- `effective_cost_per_run`
- `effective_cost_per_success`

### Cost formula

For each provider/model, calculate:

```text
effective_cost =
  billed_input_tokens * input_price
  + billed_output_tokens * output_price
  + cache_write_tokens * cache_write_price
  + cache_read_tokens * cache_read_price
```

Then compare:

- current branch
- `origin/main`

on the same scenario and same model family.

### What the current system can prove

The current optimized harness can already prove:

- cache write/read tokens are now visible
- provider is now visible
- context footprint is now measurable

That means we can compute effective cost honestly.

The old system could not do this, because cache tokens were hidden and provider was blank.

### What the future system should prove

The future system should not just show more cache activity.
It should show one of these:

- lower effective cost at equal quality
- same effective cost with materially better reliability
- same cost with materially better latency

### Current vs future difference

Current branch:

- exposes the truth needed for cost math
- does not yet prove cost savings on its own

Future target:

- uses the richer signal to actually reduce effective cost by:
  - better compaction timing
  - better session rollover
  - better tool-output shaping
  - more stable cache reuse

## 2. Speed

This bucket answers:

- Does the harness make repeated work faster?
- Does it reduce time wasted on overgrown prompts?

### Core metrics

- `wall_clock_run_time`
- `time_to_first_token`
- `time_per_turn`
- `tool_time_share`
- `retry_time_overhead`
- `compaction_time_overhead`

### What the current system can prove

The current branch can already help us correlate latency with:

- prompt footprint
- cache read/write activity
- compaction pressure

That is useful, because some latency regressions are not model regressions at all.
They come from oversized prompts or poor turn structure.

### What the future system should prove

The future system should reduce p50/p95 latency on repeated long-context workflows by:

- reusing cached prefixes
- avoiding oversized prompts
- compacting before overflow
- reducing overflow retry churn

### Current vs future difference

Current branch:

- improves observability of why a run was slow
- gives us the data needed to explain latency

Future target:

- improves actual latency through better context management

## 3. Reliability

This bucket answers:

- Does the harness fail less?
- Does it survive long sessions better?
- Does it recover gracefully near context limits?

### Core metrics

- `successful_completion_rate`
- `overflow_failure_rate`
- `compaction_retry_success_rate`
- `hard_failure_rate`
- `turn_abandonment_rate`
- `session_rollover_success_rate` when we add rollover

### What the current system can prove

The current optimized harness already provides real reliability improvements through:

- better context occupancy estimation
- reserved output headroom
- overflow-triggered compaction and retry
- better file/provider telemetry for debugging failures

This is one place where the harness already creates clear value even if cost has not improved yet.

### What the future system should prove

The future system should reduce:

- prompt-too-long failures
- late compaction failures
- runaway context growth from large tool outputs

and increase:

- long-horizon task completion rate
- successful recovery after overflow

### Current vs future difference

Current branch:

- already improves resilience near context limits

Future target:

- adds semantic compaction and session rollover so long sessions stay healthy longer

## 4. Quality

This bucket answers:

- Did the agent still do the right work?
- Did optimization hurt correctness?
- Did compaction or caching cause the model to miss important context?

### Core metrics

- `task_success_rate`
- `build_pass_rate`
- `test_pass_rate`
- `pairwise_output_preference`
- `rubric_score`
- `correct_file_change_rate`
- `regression_rate`

### What the current system can prove

The current branch can already improve quality evaluation because it now surfaces:

- actual file changes
- actual provider/model
- turn-by-turn context occupancy

That makes quality failures easier to attribute and compare.

### What the future system should prove

The future system must show that:

- compaction does not degrade useful task performance
- long-context optimization does not cause loss of important task details
- cache-aware prompting preserves or improves output consistency

### Current vs future difference

Current branch:

- improves quality observability

Future target:

- proves quality parity or improvement under longer and more demanding tasks

## The Systems We Should Compare

We should compare three states, not just two.

### 1. Baseline system

This is `origin/main`.

It answers:

- what we had before
- how much truth was missing
- what failures or blind spots existed

### 2. Current optimized system

This is the current branch with:

- cache token plumbing
- better context occupancy
- provider reporting
- file-change reporting
- better compaction triggering
- overflow compaction retry

It answers:

- whether the harness is now telling the truth
- whether reliability has improved
- whether we can compute real effective cost

### 3. Future target system

This is the next wave, not yet complete.

It should add:

- provider-aware effective cost calculation in reports
- latency capture in benchmark artifacts
- better tool-output shaping
- semantic compaction
- session rollover
- more workflow-level benchmark coverage

It answers:

- whether better telemetry and control actually become better product economics

## Recommended Eval Types

We should run four eval types that map to the four buckets.

### A. Paired direct harness evals

These are the cleanest low-noise measurements.

Use them to compare:

- baseline harness
- current harness

on the same prompts and the same disposable workspace.

Best for:

- cost
- latency
- reliability

### B. Workflow API evals

These exercise more of Aura OS:

- project import
- spec generation
- task extraction
- dev loop

Best for:

- end-to-end reliability
- product realism
- quality under orchestration

### C. Long-horizon coding evals

These should be multi-turn and tool-heavy.

Examples:

- read repo -> plan -> edit -> refine -> summarize
- same files revisited across turns
- repeated search/read/edit cycles
- large tool outputs that pressure context

Best for:

- cache value
- compaction value
- long-context quality

### D. Overflow and compaction stress evals

These are intentionally adversarial.

Examples:

- repeated large reads
- repeated search results
- repeated file diffs
- growing tool chatter

Best for:

- reliability
- compaction correctness
- overflow recovery

## The Most Honest Success Criteria

We should call the work a success if we can show:

### Strong success

- equal-or-better quality
- lower effective cost per successful run
- lower or equal latency
- lower failure rate

### Reliability success

- equal-or-better quality
- equal cost
- clearly lower overflow or long-session failure rate

### Observability success

- no proven cost reduction yet
- no proven latency reduction yet
- but we now have truthful provider/cache/context/file telemetry

This last category is still valuable, but we should label it correctly.
It is a control-plane improvement, not yet a full economics win.

## What The Current Branch Already Wins On

Based on the direct harness benchmark comparison already run:

- cache telemetry is now visible
- context occupancy is now visible
- provider is now visible
- file changes are now visible
- reliability has improved through overflow recovery

That means the current branch already wins in:

- observability
- control
- resilience

It does **not** yet prove:

- lower effective cost
- lower latency

Those are the next proof points.

## What We Should Build Next

If we want the future system to differentiate cleanly on the four buckets, the next additions should be:

1. Provider-aware cost calculation in benchmark outputs
2. Latency capture in benchmark outputs
3. A small fixed scenario suite for paired comparison
4. A compaction stress suite
5. A workflow suite for Aura OS orchestration
6. Better tool-output shaping inside the harness

That is the shortest path from:

- "the harness is more truthful"

to:

- "the harness is measurably better for cost, speed, reliability, and quality"

## A Simple Decision Rule

Use this rule for every harness optimization:

```text
If it increases observability only:
  good, but not enough

If it improves reliability without hurting quality:
  valuable

If it improves reliability and lowers effective cost or latency:
  excellent

If it increases telemetry but worsens cost, speed, and reliability:
  reject or redesign
```

That keeps us honest and keeps the harness focused on product value instead of optimization theater.
