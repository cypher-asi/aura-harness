# Context Benchmark Suite

This document describes the local benchmark suite we can use to evaluate the harness context work in a way that is practical and honest.

The suite currently lives in `aura-os` because it talks to the running local stack, but the purpose is to validate harness behavior.

## What This Suite Measures

The suite is designed to evaluate four buckets:

1. Cost
2. Speed
3. Reliability
4. Quality

It is intentionally small right now.
The goal is not to cover every edge case yet.
The goal is to give us a repeatable baseline that is good enough to catch meaningful changes.

## Current Scenarios

### 1. `harness-context-static-site`

A 4-turn static-site editing flow:

- inspect
- implement
- refine
- summarize

This is a compact incremental-edit benchmark.

### 2. `harness-context-repo-iteration`

A 5-turn repeated repo-iteration flow:

- create first version from a rich inline product brief
- refine hero and features
- add proof + FAQ
- polish CTA/footer/responsive behavior + update changelog
- summarize

This is the more important scenario for context and cache behavior because it carries a larger stable prefix through multiple turns.

## How To Run It

From `aura-os`:

```bash
cd /Users/shahrozkhan/Documents/zero/aura-os

# Current harness branch on the local stack
AURA_EVAL_RESULTS_DIR=test-results/current-harness-suite-v4 \
  ./evals/local-stack/bin/run-harness-context-suite.sh

# Baseline harness on a separate port
AURA_EVAL_HARNESS_URL=http://127.0.0.1:3414 \
  AURA_EVAL_RESULTS_DIR=test-results/baseline-harness-suite-v4 \
  ./evals/local-stack/bin/run-harness-context-suite.sh

# Compare the summaries
node ./interface/scripts/compare-benchmark-usage.mjs \
  interface/test-results/current-harness-suite-v4/aura-benchmark-usage-summary.json \
  interface/test-results/baseline-harness-suite-v4/aura-benchmark-usage-summary.json \
  harness-suite-v4-compare
```

Generated artifacts:

- `interface/test-results/current-harness-suite-v4/aura-benchmark-usage-summary.json`
- `interface/test-results/baseline-harness-suite-v4/aura-benchmark-usage-summary.json`
- `test-results/harness-suite-v4-compare.json`

## Latest Neutral Read

These results are from the current optimized harness branch compared to `origin/main`.

### Aggregate suite result

- billed input delta: `-27`
- billed output delta: `-1609`
- total run wall-clock delta: `-41435 ms`
- cache write tokens surfaced: `145440`
- cache read tokens surfaced: `640948`
- prompt footprint surfaced: `786482`

So on this suite, the current harness branch:

- finished about `41.4s` faster overall
- used fewer billed input tokens
- used fewer billed output tokens
- surfaced large cache reuse across the repeated-turn workflow

### Static-site scenario

Current branch vs baseline:

- run wall-clock delta: `-11845 ms`
- output delta: `-776`
- input delta: `-8`

This scenario got faster and smaller on the optimized branch.

### Repeated repo-iteration scenario

Current branch vs baseline:

- run wall-clock delta: `-29590 ms`
- output delta: `-833`
- input delta: `-19`

This longer scenario also got faster overall and smaller on billed input/output.

## Important Caveat About Cost

The current optimized harness now surfaces:

- cache write tokens
- cache read tokens
- estimated context occupancy

`origin/main` does not expose cache usage at the protocol boundary.

That means a direct “estimated effective cost” comparison between:

- current branch
- `origin/main`

is **not apples-to-apples yet**.

Why:

- the current branch includes prompt-cache costs in the reported usage
- `origin/main` hides them, so its apparent cost is artificially incomplete

So the honest claim is:

- we can now compute effective cost for the optimized harness
- we cannot yet make a fully fair cost claim against `origin/main` from protocol data alone

## What We Can Honestly Claim Today

We can already claim:

- better observability
- better context truthfulness
- better file-change truthfulness
- better total runtime on the current suite
- lower billed input/output tokens on the current suite

We should **not** yet claim:

- proven lower total cost vs `origin/main`

because the baseline still hides cache accounting.

## What Research Suggests

Anthropic's prompt caching pricing means:

- 5-minute cache writes cost `1.25x` normal input tokens
- cache reads cost `0.1x` normal input tokens

That means prompt caching tends to look worse for one-off work, and better once the same prefix is reused across multiple turns.

Practical interpretation:

- short runs may only show telemetry improvements
- repeated multi-turn runs are the place where economic value should emerge

This matches what we want from harness optimization work:

- better behavior for long-running coding-agent sessions
- not just prettier token logs

## Recommended Next Validation

The highest-value next benchmark is:

- current harness branch with caching enabled
- current harness branch with caching disabled

on the same suite.

That would give us a real same-system economic A/B instead of comparing against an older protocol that hides cache costs.

## Cache On Vs Cache Off

We ran that A/B on the current harness branch using the same `v4` suite.

Artifacts:

- cache on: [aura-benchmark-usage-summary.json](/Users/shahrozkhan/Documents/zero/aura-os/interface/test-results/current-harness-suite-v4/aura-benchmark-usage-summary.json)
- cache off: [aura-benchmark-usage-summary.json](/Users/shahrozkhan/Documents/zero/aura-os/interface/test-results/current-harness-suite-v4-nocache/aura-benchmark-usage-summary.json)
- compare: [harness-suite-v4-cache-on-vs-off.json](/Users/shahrozkhan/Documents/zero/aura-os/test-results/harness-suite-v4-cache-on-vs-off.json)

### Aggregate result

Current harness with caching enabled vs the same harness with caching disabled:

- billed input delta: `-346763`
- billed output delta: `+4520`
- estimated effective cost delta: `-0.391341`
- total run wall-clock delta: `+129537 ms`

So on this suite:

- **cache on was cheaper**
- **cache on was slower**

### Repeated repo-iteration scenario

- billed input delta: `-282418`
- billed output delta: `+1804`
- estimated effective cost delta: `-0.369782`
- run wall-clock delta: `+66215 ms`

This is the clearest sign of economic value:

- repeated long-horizon work reused enough prefix that cache-on reduced effective cost materially
- but total runtime still increased

### Static-site scenario

- billed input delta: `-64345`
- billed output delta: `+2716`
- estimated effective cost delta: `-0.021559`
- run wall-clock delta: `+63322 ms`

This one still got cheaper with cache-on, but only slightly.
The runtime penalty was larger than the cost savings.

## Practical Interpretation

The current evidence suggests:

- prompt caching is already giving us real cost value on repeated-turn workloads
- prompt caching is not yet giving us a speed win in this harness setup

So the neutral product read is:

- **cost win:** yes, on this same-system A/B
- **speed win:** no, not yet
- **reliability win:** neutral in this suite
- **quality:** both sides passed these scenarios

That means the next optimization target should be:

- keep the cost savings,
- reduce the runtime overhead.

The most likely levers are:

- smaller and more stable prompts
- less verbose output
- better tool-output shaping
- smarter compaction before prompts grow large
- reducing unnecessary first-turn sprawl on long prompts

## Repeatable Command

From `aura-os`:

```bash
cd /Users/shahrozkhan/Documents/zero/aura-os
./evals/local-stack/bin/run-harness-context-cache-ab.sh
```

This script:

- runs the suite against the current local harness
- starts a second no-cache harness on a separate port
- runs the same suite against that harness
- writes a comparison report

## Recommended Future Suite Additions

1. A reliability stress case that pushes closer to the context ceiling.
2. A tool-heavy repo maintenance case with larger read outputs.
3. A cache-on vs cache-off A/B lane.
4. A quality lane with stronger pass/fail checks than keyword-based heuristics.
