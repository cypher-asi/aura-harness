---
name: 'Blog post: the design of the Gate router, with SWE-Bench Pro results'
overview: Write a research-paper-style but accessible post for the Cypher site that explains Gate in simple terms, walks through each layer and stage of a turn, and reports real Compare results from the SWE-Bench Pro hard tasks. It adds an architecture diagram and benchmark charts to the site's existing `chart` figure system, in the same style as "The Industrialization of Intelligence".
todos:
- id: export-data
  content: 'Wait for Compare #37 to finish; write scripts/compare_summary.py and export Compares #28-#37 to JSON'
  status: pending
- id: judge-spend
  content: Derive per-call spend by purpose and get blind-judge verdicts from /compare show 37 (or ask user)
  status: pending
- id: data-ts
  content: Add GATE_* datasets to charts/data.ts with run-id provenance
  status: pending
- id: diagram
  content: Build gate-architecture flow diagram (FlowSteps) in Roadmap style
  status: pending
- id: charts
  content: Build gate-swe-tasks (PairedBars), gate-savings, gate-iterations, gate-spend figures and register them
  status: pending
- id: post
  content: 'Write content/posts/gate-router-design.md: abstract, layers, stage walkthrough, evaluation, validity, related work, sources'
  status: pending
- id: test
  content: Add vitest that post chart names are registered; run npm run typecheck and npm test
  status: pending
- id: verify
  content: Verify every cited symbol, default and number against aura-harness HEAD and the exported data
  status: pending
---

# Blog post: the design of the Gate router, with SWE-Bench Pro results

Write a research-paper-style but accessible post for the Cypher site that explains Gate in simple terms, walks through each layer and stage of a turn, and reports real Compare results from the SWE-Bench Pro hard tasks. It adds an architecture diagram and benchmark charts to the site's existing `chart` figure system, in the same style as "The Industrialization of Intelligence".

## Todos

- [ ] Wait for Compare #37 to finish; write scripts/compare_summary.py and export Compares #28-#37 to JSON
- [ ] Derive per-call spend by purpose and get blind-judge verdicts from /compare show 37 (or ask user)
- [ ] Add GATE_* datasets to charts/data.ts with run-id provenance
- [ ] Build gate-architecture flow diagram (FlowSteps) in Roadmap style
- [ ] Build gate-swe-tasks (PairedBars), gate-savings, gate-iterations, gate-spend figures and register them
- [ ] Write content/posts/gate-router-design.md: abstract, layers, stage walkthrough, evaluation, validity, related work, sources
- [ ] Add vitest that post chart names are registered; run npm run typecheck and npm test
- [ ] Verify every cited symbol, default and number against aura-harness HEAD and the exported data

<!-- end of generated header: edit below -->

## What I found

**How the site draws figures.** "The Industrialization of Intelligence" embeds figures with fenced blocks such as `chart` / `roadmap`. `Reader.tsx` swaps each block for `<PostChart name>`. The figures are React components in `src/features/terminal/charts/PostChart.tsx`, registered in `CHARTS`, with their numbers in `charts/data.ts`. They are HTML/CSS inside a shared `Figure` frame (title, subtitle, sources caption, tooltip on every mark), drawn with `Bars` and the stepped `Roadmap` diagram. Any new diagram or graph is therefore code in the website repo plus a name in the post.

**The real data on this machine.** It lives in `~/.aura/runs/*/{meta.json,summary.json,steps.jsonl}`. `meta.title` = `Compare #n <slot>: <task>`, `meta.arm` = `A · gate` / `B · claude-opus-5-5`, and `task_id` = `swe_bench_pro_hard/<task>`. `summary.cost_usd`, `wall_ms` and `outcome` hold the hidden-test verdict. `steps.jsonl` holds each `route` decision (model, effort, category, difficulty) and Gate's own calls by `purpose` (`classify`, `criteria`, `judge`).

SWE-Bench Pro hard Compares, counting pairs where both runs finished:

| Compare | Pairs | Gate cost | Opus cost | Gate/Opus | Pass (Gate/Opus) |
|---|---|---|---|---|---|
| #28 | 2 | $9.55 | $7.85 | 1.22× | 2/2 |
| #29 | 5 | $2.78 | $3.43 | 0.81× | 4/5 |
| #30 | 12 | $6.20 | $3.80 | 1.63× | 7/5 |
| #31 | 10 | $17.16 | $7.86 | 2.18× | 10/10 |
| #32 | 5 | $3.25 | $3.51 | 0.93× | 4/5 |
| #33 | 5 | $4.77 | $3.75 | 1.27× | 5/5 |
| #36 | 4 | $1.23 | $3.09 | 0.40× | 4/4 |
| #37 | 14 of 16 | $5.81 | $12.04 | 0.48× | 14/14 |

Compare #37 started at 01:14, after `d07d3e3a` (01:12), so it ran the current design. It was still running when I checked: 2 flipt tasks had not finished. Every Gate run in #37 went to Sonnet 5.5 (one also moved up to Opus), and none dropped to Haiku. Earlier Compares ran earlier designs, and the CHANGELOG ties several fixes to them (#31: max-effort retries on Opus; #32: medium work sent to Haiku).

## Decisions (say if you want any changed)

- **Headline = Compare #37** (current design, 16 tasks across ansible, element-web and flipt), with #36 as a second run on the same design. Per-task graphs come from #37.
- **Earlier Compares are shown as an iteration chart**, not hidden. The cost ratio by Compare, each annotated with the design change that came before it, is the research-paper part: it shows what each fix bought. It also keeps the savings claim honest, since early versions cost more than Opus.
- **Opus 5.5 alone is the baseline**, at list prices, and Gate's cost includes its own classifier, criteria and judge calls. This holds from `f156b01d`/`a1ab2bfe` on; I'll check that per run.
- **Blind-judge quality** comes from `/compare show 37`. That data is in the RocksDB record, not the run bundles. If I can't read it, I'll ask you to paste the output; if it isn't available, the post reports hidden-test pass rates only.

## Deliverables

| Repo | File | What |
|---|---|---|
| aura-harness | `scripts/compare_summary.py` | Read-only exporter: `AURA_HOME/runs` → JSON per Compare/task/arm (cost, wall, outcome, routes, Gate call counts and cost by purpose). Makes every number in the post reproducible. |
| website | `content/posts/gate-router-design.md` | The post, with front matter (`title`, `slug`, `summary`) like the fixture, and a numbered `## Sources` list. |
| website | `src/features/terminal/charts/data.ts` | `GATE_FLOW`, `GATE_SWE_TASKS`, `GATE_TOTALS`, `GATE_ITERATIONS`, `GATE_SPEND`, each noting the Compare numbers and run ids they come from. |
| website | `src/features/terminal/charts/PostChart.tsx` + `.module.css` | Five new figures, registered in `CHARTS`. |
| website | a small vitest | Every `chart` block in the post names a registered chart (`isChart`). |

## Figures

1. `gate-architecture`: the diagram, in the Roadmap's style (steps in a `Figure`, mono labels, one accent, tooltips that list what each stage does and its module). The stages are Classify → Pick → Criteria → Work → Verify → Judge → Escalate, with a "Learn" return band that feeds the table back into Pick. Below them sit the three layers: pure `aura-agent-gate`, the event loop in `aura-app`, and the kernel and its record.
2. `gate-swe-tasks`: per task in #37, paired horizontal bars, Gate (accent) vs Opus cost. Each pair shows a pass/fail mark. Tooltips give wall time, routed model and effort, and Gate's own calls.
3. `gate-savings`: overall #37 (and #36) totals, with cost, wall time and pass rate per arm, and the saving in % with a 1.0× marker.
4. `gate-iterations`: Gate/Opus cost ratio by Compare #28→#37 with a 1.0× marker. Each bar's tooltip names the commits before it.
5. `gate-spend`: where Gate's dollars go in #37: the work model, escalations, and classifier, criteria and judge overhead. Included only if per-call usd can be derived from `log.json`/usage plus list prices.

`Bars` already handles 3–5. Figure 2 needs a paired variant (`PairedBars`). Figure 1 is a new `FlowSteps` layout next to `RoadmapSteps`.

## Post structure (about 3,000 words)

- **Abstract** (one paragraph): the result in numbers.
- **1. The idea in simple terms**: one model for everything overpays or underdelivers. Gate starts cheap, checks the work, moves up only when it fails, and learns which model is good enough for each kind of work. An analogy and the route line `gate → model (route) at effort · category, reason`.
- **2. Architecture in layers** → `chart gate-architecture`. Pure decision logic with no I/O; the event loop that runs it; the kernel and append-only record that make it auditable and replayable.
- **3. Walking through a turn**, one subsection per stage, each with what it does, why, and the module: 3.1 candidates and tiers · 3.2 classify · 3.3 pick and effort · 3.4 acceptance criteria · 3.5 the work (and leading with subtasks) · 3.6 verify (checks, follow-ups, disowned passes, own-tests-only, review) · 3.7 judge · 3.8 escalate · 3.9 learn. Each subsection is illustrated with a real route or verdict line taken from a #37 run.
- **4. Evaluation**
  - **Method**: SWE-Bench Pro HARD-51, hidden tests in Docker, isolated worktrees, list prices, Gate's own calls counted.
  - **Results**: `chart gate-savings`, then `chart gate-swe-tasks`, then `chart gate-spend`.
  - **How the design got here**: `chart gate-iterations`, with 3–4 short "what broke, what we changed" notes taken from the CHANGELOG.
- **5. Threats to validity**: small samples, one repeat per task, a single provider (Claude plan), the pass-rate ceiling when both arms pass, list price vs plan usage, and Gate still changing.
- **6. Related work**: FrugalGPT, RouteLLM, cascades (links checked before citing).
- **7. Limits and next steps**, then a settings appendix (`AURA_GATE_*`, defaults from `aura-config/src/gate.rs`).
- **Sources**: a numbered list like the fixture's, using the `[n](url)` citations the site indexes.

## Risks

- #37 is still running, so the numbers are frozen only after it finishes. Unfinished pairs are left out and the post says so.
- Small n: the post states sample sizes on every figure.
- Gate changes daily. The post pins the commit (`d07d3e3a`) that the results ran on.
- The `aura` company has no `siteId` in `src/lib/site.ts`, so publishing it under Aura needs a site created first. Publishing is out of scope.
