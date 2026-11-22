---
name: 'Blog post: the design of the Gate router, with SWE-Bench Pro results and prior work'
overview: Write a post for the Cypher site in the style of a research paper that stays accessible. It explains Gate in simple terms, places it among published routing approaches with numbered citations, walks through each layer and stage, and reports real Compare results from the SWE-Bench Pro hard tasks. It adds an architecture diagram and benchmark charts to the site's existing `chart` figure system.
todos:
- id: research
  content: Research prior work via arXiv API and web_fetch; verify each source and write a source table with notes
  status: completed
- id: export-data
  content: 'Wait for Compare #37 to finish; write scripts/compare_summary.py and export Compares #28-#37'
  status: completed
- id: judge-spend
  content: Derive per-call spend by purpose and get blind-judge verdicts from /compare show 37
  status: completed
- id: data-ts
  content: Add GATE_* datasets to charts/data.ts with run-id provenance
  status: completed
- id: diagram
  content: Build gate-architecture flow diagram (FlowSteps) in Roadmap style
  status: completed
- id: charts
  content: Build gate-savings, gate-swe-tasks (PairedBars), gate-spend, gate-iterations and register them
  status: completed
- id: post
  content: 'Write the post: abstract, simple idea, how others route + comparison table, layers, walkthrough, evaluation, validity, sources'
  status: completed
- id: test
  content: Add vitest for chart names; run npm run typecheck and npm test
  status: completed
- id: verify
  content: Verify every citation, symbol, default and number against sources, aura-harness HEAD and exported data
  status: completed
---

# Blog post: the design of the Gate router, with SWE-Bench Pro results and prior work

Write a post for the Cypher site in the style of a research paper that stays accessible. It explains Gate in simple terms, places it among published routing approaches with numbered citations, walks through each layer and stage, and reports real Compare results from the SWE-Bench Pro hard tasks. It adds an architecture diagram and benchmark charts to the site's existing `chart` figure system.

## Todos

- [x] Research prior work via arXiv API and web_fetch; verify each source and write a source table with notes
- [x] Wait for Compare #37 to finish; write scripts/compare_summary.py and export Compares #28-#37
- [x] Derive per-call spend by purpose and get blind-judge verdicts from /compare show 37
- [x] Add GATE_* datasets to charts/data.ts with run-id provenance
- [x] Build gate-architecture flow diagram (FlowSteps) in Roadmap style
- [x] Build gate-savings, gate-swe-tasks (PairedBars), gate-spend, gate-iterations and register them
- [x] Write the post: abstract, simple idea, how others route + comparison table, layers, walkthrough, evaluation, validity, sources
- [x] Add vitest for chart names; run npm run typecheck and npm test
- [x] Verify every citation, symbol, default and number against sources, aura-harness HEAD and exported data

<!-- end of generated header: edit below -->

## What I found

**How the site cites sources.** The live "Industrialization of Intelligence" post cites sources inline as `[n](url)`, which the site renders as superscripts, and ends with a numbered `## Sources` list: `Author/Org, [Title](url), venue, date.` Each URL is indexed into `research_sources` and shows in the Sources panel, so the Gate post must follow that format exactly.

**How the site draws figures.** A post embeds a figure with a fenced block (`chart` / `<name>`). `Reader.tsx` swaps the block for `<PostChart name>`. The figures are React components in `src/features/terminal/charts/PostChart.tsx`, registered in `CHARTS`, with their data in `charts/data.ts`. They are HTML/CSS inside a shared `Figure` frame (title, subtitle, sources caption, tooltips), built from `Bars` and the stepped `Roadmap` diagram. New diagrams and graphs are therefore code in the website repo.

**The benchmark data** is in `~/.aura/runs/*/{meta,summary,steps}`: title `Compare #n <slot>: <task>`, arm, `swe_bench_pro_hard/<task>`, `cost_usd`, `wall_ms`, `outcome` (hidden tests), `route` decisions, and Gate's own calls by `purpose`. Pairs where both runs finished:

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

#37 ran on `d07d3e3a`, the current design, and was still running (2 tasks) when I checked.

**Prior work.** The arXiv API (`export.arxiv.org/api/query`) works through `web_fetch`; `web_search` is not configured. I confirmed these IDs exist and are relevant:

| Thread | Sources confirmed so far |
|---|---|
| Cascades (try cheap, defer on doubt) | FrugalGPT 2305.05176; Mixture-of-Thoughts cascades 2310.03094 (ICLR 2024); token-level deferral 2404.10136 |
| Learned pre-hoc routers | RouteLLM 2406.18665 (+ LMSYS blog); Hybrid LLM 2404.14618; AutoMix 2310.12963 (self-verification) |
| Router benchmarks and theory | RouterBench 2403.12031; routing survey 2502.00409 |
| Routing as policy or RL | Router-R1 2506.09033; Arch-Router 2506.16655 (preference-aligned) |
| Agentic and coding routing (closest to Gate) | TwinRouterBench 2605.18859 (step-level routing on SWE-bench, downgrade-and-cascade); Triage 2604.07494 (Haiku/Sonnet/Opus tiers and a verification gate on SWE-bench Lite); "When the Cheapest Feasible Router Fails" 2611.20417 (deployment commitment, which matches Gate's min-tries/target promotion) |
| Judging and retry-with-feedback | LLM-as-a-judge / MT-Bench 2306.05685; Reflexion 2303.11366 |
| Exploration with logged propensities | contextual bandits, Li et al. 1003.0146 (Gate logs `explored`/`propensity` for offline replay) |
| Benchmark | SWE-Bench Pro 2509.16941 + `scaleapi/SWE-bench_Pro-os` |

Still to find and verify by fetching each URL: industry posts. Candidates are OpenAI's GPT-5 system card (real-time router), Anthropic's "Building effective agents" (routing workflow), OpenRouter Auto Router docs, Not Diamond and Martian, and Anthropic's model pricing page for list prices. I'll also look for 2026 papers on cost-aware agent routing.

## Approach to the related-work section

- An early **"2. How others route"** section. It keeps the post's tone: short paragraphs, one thread each, 1–3 citations per thread.
- It ends with a GFM **comparison table** that places Gate on five axes: when it decides (before the work vs after a failure), what signal it routes on (learned score, uncertainty, verification), how it learns (offline training vs an online fold), unit (query vs turn or subtask), and how it verifies (none, self-check, tests plus a judge).
- The **claim to make, and back with citations**: most routers decide once, before the work, from a model trained on preference or benchmark data, and are scored on one-shot prompts (RouterBench, RouteLLM). Gate is a verified cascade for agent turns that learns online from its own record. It is closest to TwinRouterBench's downgrade-and-cascade protocol and to Triage's tiers behind a verification gate. "The cheapest feasible router fails" is the risk Gate's promotion thresholds and judge address.
- Every citation is fetched and checked (title, authors, date) before it goes in. There are no uncited claims about others' results.

## Decisions (as before; say if you want any changed)

- **Headline = Compare #37** (current design), with #36 as a second run. Per-task graphs come from #37.
- **Earlier Compares are shown as an iteration chart**, each with the design change before it, so the savings claim stays honest.
- **Baseline = Opus 5.5 alone** at list prices. Gate's own classifier, criteria and judge calls are included.
- **Blind-judge verdicts** come from `/compare show 37`, which you paste if I can't read them from the record.

## Deliverables

| Repo | File | What |
|---|---|---|
| aura-harness | `scripts/compare_summary.py` | Read-only exporter: `AURA_HOME/runs` → JSON per Compare/task/arm. Makes every number reproducible. |
| website | `content/posts/gate-router-design.md` | The post, with front matter like the fixture, inline `[n](url)` citations and a numbered `## Sources` list. |
| website | `charts/data.ts` | `GATE_FLOW`, `GATE_SWE_TASKS`, `GATE_TOTALS`, `GATE_ITERATIONS`, `GATE_SPEND`, each with its Compare and run ids. Figure `sources` point at SWE-Bench Pro and the harness. |
| website | `charts/PostChart.tsx` + `.module.css` | Five new figures registered in `CHARTS`: `FlowSteps` and `PairedBars`, plus reuse of `Bars`. |
| website | vitest | Every `chart` block in the post is registered (`isChart`). |

## Figures

1. `gate-architecture`: the flow diagram in the Roadmap's style. The stages are Classify → Pick → Criteria → Work → Verify → Judge → Escalate, with a Learn loop back into Pick and the three layers underneath. Tooltips list what each stage does and its module.
2. `gate-savings`: totals for #37 (and #36): cost, wall time and pass rate per arm, the saving in %, and a 1.0× marker.
3. `gate-swe-tasks`: per task, paired Gate vs Opus cost bars with pass marks. Tooltips give the model and effort, wall time and Gate's own calls.
4. `gate-spend`: Gate's dollars split into the work model, escalations, and classifier, criteria and judge overhead (only if per-call cost can be derived).
5. `gate-iterations`: Gate/Opus cost ratio by Compare #28→#37. Tooltips name the commits before each one.

## Post structure (about 3,500 words, 25–35 sources)

- **Abstract**: the result in numbers.
- **1. The idea in simple terms.** An analogy and the route line.
- **2. How others route.** Cascades; learned routers; benchmarks; RL and preference routers; agentic and coding routing; judges and feedback. Ends with the comparison table.
- **3. Architecture in layers** → `gate-architecture`.
- **4. Walking through a turn**: 4.1 candidates and tiers · 4.2 classify · 4.3 pick and effort · 4.4 criteria · 4.5 work and leading · 4.6 verify · 4.7 judge · 4.8 escalate · 4.9 learn. Each step uses a real line from a #37 run and links back to the related work it borrows from or differs from.
- **5. Evaluation**: method (SWE-Bench Pro HARD-51, Docker hidden tests, worktrees, list prices); results → `gate-savings`, `gate-swe-tasks`, `gate-spend`; how the design got here → `gate-iterations`.
- **6. Threats to validity.**
- **7. Limits and next steps.**
- **Appendix**: `AURA_GATE_*` settings.
- **Sources**: a numbered list.

## Risks

- #37 must finish before the numbers are frozen. Unfinished pairs are left out and the post says so.
- Small n: every figure states its n.
- Some sources are dated 2026 and could not be checked from training knowledge. Each is fetched and checked, and dropped if it doesn't resolve.
- Gate keeps changing. The post pins `d07d3e3a`.
- Publishing under Aura needs a site created first (no `siteId` in `src/lib/site.ts`). Publishing is out of scope.
