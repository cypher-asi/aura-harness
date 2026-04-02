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

## Clean-State Validation

After cleaning both working trees and rerunning the direct live benchmark on the
same local stack, we also captured a cleaner, narrower A/B on the
`harness-context-static-site` scenario alone.

Artifacts:

- cache on: [aura-benchmark-usage-summary.json](/Users/shahrozkhan/Documents/zero/aura-os/interface/test-results/clean-post-merge-cache-on/aura-benchmark-usage-summary.json)
- cache off: [aura-benchmark-usage-summary.json](/Users/shahrozkhan/Documents/zero/aura-os/interface/test-results/clean-post-merge-cache-off/aura-benchmark-usage-summary.json)
- compare: [clean-post-merge-cache-on-vs-off.json](/Users/shahrozkhan/Documents/zero/aura-os/interface/test-results/clean-post-merge-cache-on-vs-off.json)

### Clean static-site result

Cache on vs cache off:

- billed input delta: `-258205`
- billed output delta: `-1482`
- estimated effective cost delta: `-1.212202`
- total run wall-clock delta: `-49764 ms`
- prompt input footprint delta: `-162099`
- max context utilization delta: `-0.1092`

This is the strongest clean proof we have so far for the current V1 work:

- **cache on was cheaper**
- **cache on was faster**
- **cache on kept context pressure lower**
- **both runs still passed quality**

## Practical Interpretation

The current evidence suggests:

- prompt caching is already giving us real cost value on repeated-turn workloads
- prompt caching is not yet giving us a speed win in this harness setup

The newer clean static-site A/B shows that this can swing the other way on a
more focused workload:

- cache on was both cheaper and faster in that clean run

So the most honest current read is:

- repeated-turn economics are clearly better with caching
- runtime behavior depends on workload shape
- we should keep using both suite-level and focused clean A/B runs

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

## Follow-up Experiments We Rejected

After this A/B, we tried two follow-up changes aimed at lowering the runtime penalty.

### 1. Leaner system prompt

Goal:

- reduce prompt size
- reduce planning chatter
- improve runtime

Result:

- runtime improved a lot
- effective cost also improved
- but one scenario failed the quality check

Why we rejected it:

- it traded away instruction quality and task reliability
- that is not acceptable for a coding-agent harness

Neutral read:

- a smaller prompt is not automatically a better prompt
- for harness work, preserving output quality matters more than winning a single latency number

### 2. Removing the automatic cache breakpoint on the last user message

Goal:

- reduce cache overhead on dynamic user content
- improve runtime while keeping quality stable

Result:

- runtime improved compared to the earlier cache-on baseline
- quality still passed
- but billed input and effective cost got materially worse

Why we rejected it:

- it made the system faster by shifting more prompt work back into expensive billed input
- that is not a clean product win

Neutral read:

- Anthropic prompt caching works best when stable reusable prefixes are clearly marked
- removing a breakpoint from the end of the conversational prefix reduced reuse more than it helped runtime

## Current Keep/Reject Decision

Keep:

- truthful cache read/write reporting
- better context occupancy estimation
- provider and file-change reporting
- compaction with reserved output headroom
- overflow recovery and bounded retry
- prompt caching toggle for A/B evaluation
- benchmark suite and comparison tooling

Reject for now:

- leaner prompt variant that hurts quality
- cache-breakpoint removal that improves runtime but worsens cost

## Current Recommendation

The current best validated state is still the committed `v4` cache-on implementation plus the cache-on vs cache-off A/B runner.

That means our next optimization target should not be:

- "use less prompt text at any cost"
- "remove cache markers and hope runtime improves"

It should be:

- reduce redundant tool output and read-heavy prompt growth
- improve long-session prompt shaping without weakening instructions
- keep measuring cost, speed, reliability, and quality together

## Targeted Cached-Read Microbenchmark

We also added a smaller harness-layer optimization aimed at repeated read-heavy cache hits.

What it does:

- keeps first-time tool results unchanged
- shortens only large repeated cache hits for cacheable read-only tools
- leaves write tools and non-cacheable tools untouched

This is intentionally narrower than semantic compaction.
It is meant to reduce prompt bloat from re-inserting the same large read result over and over.

### Verified result

The deterministic harness test covers a repeated `read_file` cache hit with a `9000` character payload.

Measured outcome:

- shaped cached result stays at or below `4300` characters
- at least `4500` characters are removed from the repeated prompt payload
- using the harness `4 chars ≈ 1 token` heuristic, that is at least about `1125` prompt tokens saved per repeated hit

We also checked the other shaped read-heavy tools with deterministic payloads:

- repeated `search_code` cache hit with `6000` characters:
  - shaped result stays at or below `2300` characters
  - at least `3500` characters removed
  - about `875` prompt tokens saved per repeated hit
- repeated `list_files` cache hit with `3000` characters:
  - shaped result stays at or below `1400` characters
  - at least `1500` characters removed
  - about `375` prompt tokens saved per repeated hit

That gives us a simple working range for the current optimization:

- roughly `375` to `1125` prompt tokens saved per repeated large cache hit depending on tool type

This is not yet a full product benchmark.
It is a lower-layer prompt-footprint benchmark for the exact path we changed.

### Honest status

- the harness-layer reduction is real and tested
- the dedicated live eval scenario for this exact optimization still needs work before we should trust it as an end-to-end product benchmark

So the neutral read is:

- this looks like a good low-risk prompt-footprint optimization
- we should keep validating it in broader live evals before making larger cost/latency claims

We also verified the repeated-turn effect directly at the message-history level:

- a two-turn repeated `read_file` replay with a `9000` character cached payload saves at least `4500` characters from the reinserted message history
- using the harness heuristic, that is again about `1125` prompt tokens saved on the repeated turn itself

That matters because it confirms the saving is not just a string helper detail.
It actually reduces the message footprint that the next model call sees.

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

## Direct Runner Note

If you run the Node benchmark directly instead of the wrapper scripts, you must
source the local auth token first or the live harness will fail in proxy mode.

Example:

```bash
cd /Users/shahrozkhan/Documents/zero/aura-os
set -a
source ./evals/local-stack/.runtime/auth.env
set +a

cd interface
AURA_EVAL_RESULTS_DIR=test-results/manual-run \
  AURA_EVAL_SCENARIO_ID=harness-context-static-site \
  node ./scripts/run-harness-context-benchmark.mjs
```

Without that token, the live harness benchmark can fail with:

- `Proxy mode requires a JWT auth token`

That is a benchmark setup issue, not a harness regression.

## Recommended Future Suite Additions

1. A reliability stress case that pushes closer to the context ceiling.
2. A tool-heavy repo maintenance case with larger read outputs.
3. A cache-on vs cache-off A/B lane.
4. A quality lane with stronger pass/fail checks than keyword-based heuristics.
