---
name: 'Gate article: architecture and cost, then judge and learning deep dives'
overview: 'Write docs/articles/gate.md. Its main body is Gate''s architecture, request flow and routing approach, together with the cost comparison against single-model baselines. Two deep dives follow: how a failed turn is judged, and how the routing table learns online with no training run.'
todos:
- id: fresh-run
  content: 'Rerun the benchmarks cleanly: gate_cost and gate_hard for the opus, haiku and gate arms (3 repeats), and judge.py for Gate against Opus; keep the stderr logs'
  status: pending
- id: verify-data
  content: Aggregate cost, pass rate, wall time and calls per arm and per category; the Gate cost breakdown (classifier, work, subtasks, judge, escalations); judge and escalation metrics; /gate snapshots
  status: pending
- id: write-architecture
  content: 'Write Parts 1-2: what Gate is, the layering diagram, the one-turn sequence diagram, 9 flow steps with their files'
  status: pending
- id: write-cost
  content: 'Write Part 3: the approach, where the cost goes, the gate_cost and gate_hard comparison tables, blind-judge quality, the per-category view, how to read the numbers'
  status: pending
- id: write-judge
  content: 'Write Part 4, the judge deep dive: failure signals, trigger rules, prompt contents, request and parsing, three-outcome flowchart, subtask variant, cost discipline, metrics'
  status: pending
- id: write-learning
  content: 'Write Part 5, the learning deep dive: record fold, signals, superseding rules, live observer path, promotion/rule-out/explore, worked example, limits'
  status: pending
- id: settings-link
  content: Write the Part 6 settings table and link the article from README.md and docs/cli.md#gate
  status: pending
---

# Gate article: architecture and cost first, deep dives second

## Deliverable
One new file, `docs/articles/gate.md` (about 3,500 words), linked from `README.md` and `docs/cli.md#gate`. No code changes. Diagrams are mermaid. Functions are cited by name, not line number. No history of earlier designs.

**Weighting:** Parts 1–3 are the core (about 60%). Parts 4–5 are the deep dives (about 35%). Part 6 is the reference.

## Structure

### Part 1. What Gate is (short)
`gate` is a model id that resolves to one concrete model per turn and per subtask. It picks the cheapest model proven good enough for the kind of work, moves work up a tier when it fails, and shows every choice (`gate → model (route) · category, reason`).

### Part 2. Architecture and flow (core)
- **Layering diagram:** `aura-agent-gate` holds the pure logic (candidates, routing, table, classifier and judge request builders, outcome rules, lead limits). `aura-app` (`event_loop/`, `headless/`) runs it. Every model call goes through `Kernel::reason`. Invariant: Gate does no I/O and never holds a provider. Gate only narrows permissions.
- **One-turn sequence diagram:** prompt → classify → pick → run → outcome → (on failure) judge → escalate → table update.
- **Flow steps, each with a sentence or two and its files:**
  1. Candidates (`router::candidates`, `ModelSelection::gate_candidates`): what the sign-ins allow, one route per model with the plan first, each with a tier.
  2. Classify (`classify.rs`, `gate_record::classify_prompt`): the cheapest fast model, 8 categories, 20s timeout.
  3. Pick (`router::pick`, `by_expected_cost`): cheapest proven, else the category's start tier, plus 5% exploration. Short here, with a pointer to Part 5.
  4. Lead or do (`lead.rs`, `event_loop/lead.rs`): only `multi_file_change` and `planning` are led. The lead has no write tools and hands work to subtasks with a `check`. Other turns are done directly.
  5. Subtasks (`gate_subtask.rs`, `task_check.rs`, `router::below_lead`): routed from their own rows. A frontier lead's subtasks go below frontier. Writers run serially per workspace.
  6. Outcome (`outcome::turn_signal`, `checks::last_check_seen`): error, checks_failed, checks_passed or none.
  7. Failure handling: judge, then escalate. One paragraph here, with a pointer to Part 4.
  8. Learning: the table is updated from the record. One paragraph, with a pointer to Part 5.
  9. Measurement surfaces: `/gate`, `/compare`, `aura eval run --model gate`.

### Part 3. Approach and cost differences (core)
- **The approach in one paragraph:** start cheap, prove a model per category, escalate on failure, pay the judge only on failures, delegate only when the work is big enough to amortize a second system prompt and a second file read.
- **Where the cost goes under Gate,** as a stacked breakdown: the classifier call per prompt, the work model, subtasks, the judge (failures only), and escalated retries. Explain each as overhead or savings.
- **Comparison against single-model arms** (gate_cost suite, list prices, hidden checks):

| Arm | Runs | Pass | Total | Mean/run | Median/run | Mean wall |
|---|---|---|---|---|---|---|
| Opus 5.5 alone | 60 | 60 | $2.64 | $0.044 | $0.035 | 64.9s |
| Haiku 5.5 alone | 60 | 60 | $0.097 | $0.0016 | $0.0014 | 49.0s |
| Gate | 42* | 42 | $0.96 | $0.023 | $0.014 | 27.0s |

  *Preliminary. Replace with a clean 3×20 run. Add the same table for `gate_hard` (Haiku currently 24/24 for $0.046; Opus and Gate are still to run).
- **Quality alongside cost:** blind pairwise judge results (Gate vs Opus), win/loss/tie per category, from a fresh `judge.py` run.
- **Per-category view:** where Gate lands on each category (fast vs standard vs frontier) and the cost per category against Opus.
- **Reading the numbers:** a fast model passes every gate_cost check, so pass rates hit a ceiling there. gate_hard and the blind judge are what separate the arms. List prices are not plan-window usage; mention `/compare`'s plan-share view.

### Part 4. Deep dive: judging a failed turn
Sources: `turn_judge.rs`, `outcome.rs`, `checks.rs`, `router::escalate`; `turns.rs::run_attempts`, `gate_record.rs::{judge_for, judge_failed}`, `task_check.rs::takeover_prompt`; the subtask path in `subagents.rs` and `headless/run.rs`.
- **What counts as failed:** `turn_signal` and `last_check_seen` (only checks that ran to completion count). `calls_for_stronger` covers Error and ChecksFailed.
- **When the judge is asked:** `judge_for`, meaning the judge is enabled, the work failed, and the model is below the judge's tier (`turn_judge::judges`). The judge is the strongest Opus, else the strongest frontier.
- **What it sees:** the request (8,000 characters), the file-tool diff from `FileCheckpoints::begin_attempt` (shell changes not shown), the final reply, and the failure tail (3,000 characters). The model is not named. Quote `SYSTEM_PROMPT` with its MODEL/OTHER criteria and "When you cannot tell, answer MODEL".
- **Request shape:** no tools, thinking off, 512 tokens, Auxiliary, through `Kernel::reason`, so it is recorded.
- **Parsing:** strip `<think>` content and take the last `VERDICT:` line, with the text before it as the reasons. Anything else gives `None`.
- **Flowchart of the three outcomes:** `judged_fail` counts and escalates with `takeover_prompt`; the takeover is recorded as a prompt with reason `escalate`, and the loop repeats until the work succeeds or nothing is stronger. `judged_excused` (`voids()`) cancels the failure and the turn ends. No verdict or judge off means the failure stands and the turn escalates.
- **Subtask variant:** an excused failure gets no retry. At fault means a git snapshot restore and one retry on the next tier with the judge's reasons.
- **Cost discipline:** the judge costs $0 on clean runs and is reported separately (`Judged::cost_usd`, the chat line, the eval per-task line, the stream-json `judged` event).
- **Design notes:** it judges attribution, not correctness. Leaning toward MODEL keeps the table conservative. Escalated work is never counted.
- **Metrics:** how often the judge was called, the fault/excused/no-verdict split, cost per call, its share of the Gate arm, and success after escalation.

### Part 5. Deep dive: learning online, with no training run
Sources: `stats.rs` (`GateStats`, `Row`, `Counted::supersedes`), `outcome.rs::{effective, is_retry_stronger, is_repeat}`, `router.rs::{pick, by_expected_cost}`, `aura-core-types/src/types/gate.rs` (`OutcomeSignal`), `gate_live.rs`, `harness.rs` (`FanoutObserver`), `gate_record.rs::{note_retry, note_rewind}`.
- **The key idea:** the table is a fold over the append-only record. There are no weights and no offline fit, and replaying the record rebuilds the same table.
- **What is recorded:** `PromptPayload` (category, route, explored, propensity), then `Outcome`s. A signal table (success / failure / nothing / cancels earlier).
- **Later re-judging signals:** the judge, `/rewind`, a resend on a stronger tier (≥90% Levenshtein match), a redone subtask, `/compare pick`. All are appended, never edited.
- **Which outcome counts:** a rating beats automatic signals, otherwise the latest seq wins. A superseding outcome moves the count between rows. Escalated prompts are ignored. Order and duplicates do not matter (cite the `order_and_repeats_do_not_change_the_table` test).
- **Live path diagram:** kernel commit → `FanoutObserver` → `GateLive::on_entry` → `GateStats::observe` → the next `pick`. Startup: `rebuild_in_background` in batches of 500 on a blocking thread.
- **How the table drives routing:** promotion at ≥10 tries and ≥85% success; early rule-out via `Row::can_still_reach`, where 2 losses in 2 tries rule a model out (at most 8/10 is reachable); the start tier moving up; observed-cost ordering and list-price scaling for untried models; 5% exploration toward the nearest model not yet ruled out.
- **Worked example:** a `small_change` row over about 15 turns, with a judged_fail, a judged_excused (no change), and a rewind flipping a pass, and the pick changing. Labeled illustrative unless it is built from a real `/gate` snapshot.
- **Why no training run is needed,** and the limits: classifier errors, `none` outcomes teach nothing, global thresholds, no decay.

### Part 6. Settings reference
`AURA_GATE_TARGET`, `_MIN_TRIES`, `_EXPLORE`, `_DELEGATE`, `_JUDGE`, with their defaults.

## Data work
- Clean reruns: gate_cost and gate_hard for the opus, haiku and gate arms (3 repeats), and `judge.py --reference opus --arm gate`. Keep the stderr logs, which hold the per-task judge cost and routed models.
- From the run records: per-category model picks, judge calls and verdicts, escalation counts, the classifier cost share, and `/gate` snapshots across repeats.
- Check Haiku's cache-read pricing. Leave out runs broken by 401 errors.

## Risks
- The current Gate numbers are preliminary (42 uneven runs, no judge data) and must be labeled so until the rerun.
- Code references are by function name, to survive line drift.
